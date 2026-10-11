//! `bead init`: the tracker for this repository, optionally with a tracked `.beadroll.json` and an
//! imported Beads export.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
use std::path::{Path, PathBuf};

use beadroll_core::code::Code;
use beadroll_core::json::{self, Format, Object, Value};
use beadroll_core::text::repr;
use beadroll_git::{Fact, exists, realpath, without_password};
use beadroll_store::{
    Settings, Workspace, derived_shaped, find_workspace, uuid7, view, write_atomic,
};
use beadroll_sync::{Error as SyncError, Link, publish_soon, synchronise, writing};

use super::import::import_records;
use super::{Context, Failure, Outcome, text};

/// `init`'s arguments.
pub(crate) struct InitArgs {
    pub(crate) prefix: Option<OsString>,
    pub(crate) import_path: Option<OsString>,
    pub(crate) tracked: bool,
    pub(crate) remote: Option<OsString>,
    pub(crate) offline: bool,
}

/// `os.path.relpath(path, start)`: `path` from `start`, both absolute.
fn relpath(path: &Path, start: &Path) -> PathBuf {
    let parts = |p: &Path| -> Vec<Vec<u8>> {
        p.as_os_str()
            .as_bytes()
            .split(|b| *b == b'/')
            .filter(|part| !part.is_empty())
            .map(<[u8]>::to_vec)
            .collect()
    };
    let (to, from) = (parts(path), parts(start));
    let common = to.iter().zip(&from).take_while(|(a, b)| a == b).count();
    let mut out: Vec<Vec<u8>> = vec![b"..".to_vec(); from.len() - common];
    out.extend(to[common..].iter().cloned());
    if out.is_empty() {
        return PathBuf::from(".");
    }
    PathBuf::from(OsString::from_vec(out.join(&b'/')))
}

/// A prefix from a name: lower-cased, every run of anything but ASCII letters and digits a dash,
/// dashes trimmed; `bead` when nothing is left.
fn default_prefix(name: &str) -> String {
    let lowered = name.to_lowercase();
    let mut out = String::new();
    let mut in_run = false;
    for c in lowered.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
            in_run = false;
        } else if !in_run {
            out.push('-');
            in_run = true;
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "bead".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// The remote `init` was given, without a password: a path typed relative to the cwd that is
/// not a configured remote's name becomes absolute, or relative to the top of the checkout when it
/// goes into the tracked configuration (a stored one is read from there). Whether a password was
/// dropped.
fn remote_argument(
    settings: &Settings,
    remote: &OsStr,
    tracked: bool,
) -> Result<(OsString, bool), Failure> {
    let (git, cwd) = (&settings.git, &settings.cwd);
    let (mut remote, had_password) = without_password(remote);
    let unnamed = || -> Result<bool, Failure> {
        let named = git
            .git([OsStr::new("remote"), OsStr::new("get-url"), &remote])
            .run()?;
        Ok(!named.success())
    };
    if !remote.as_bytes().starts_with(b"/") && exists(&remote, cwd) && unnamed()? {
        let real = realpath(&remote, cwd);
        let top = settings.fact(Fact::Toplevel)?;
        let top_dir = top.success().then(|| {
            let shown = String::from_utf8_lossy(&top.stdout);
            let shown = beadroll_core::text::strip(&shown).to_owned();
            realpath(OsStr::new(&shown), cwd)
        });
        remote = match (&top_dir, tracked) {
            (Some(top_dir), true) => relpath(&real, top_dir).into_os_string(),
            _ => real.into_os_string(),
        };
    }
    Ok((remote, had_password))
}

/// The tracker `init` joined: the remote that published it and how many events it brought.
struct Joined {
    pulled: i64,
    remote: String,
}

/// What the remote holds of this repository's tracker when `init` finds none here.
enum Published {
    /// One, now pulled here (adopted, as `bead sync` adopts it, when this checkout's id is derived
    /// from its URL and the tracker was published under another).
    Joined(Joined),
    /// None, or the remote could not be asked: `init` starts the tracker.
    Nothing,
}

/// Asks the remote before `init` starts a tracker here, so that a repository never gets a second
/// one: a tracker the remote already publishes is joined as `bead sync` joins it, and one published
/// under an id this checkout cannot adopt is refused (`WORKSPACE_MISMATCH`, saying how to name it).
/// Once the remote has said it publishes one, any failure to join it fails `init`, which then
/// starts nothing, so nothing is written where a later command would not look (an adoption may
/// already have been remembered). With a tracker here already, `--offline` or no remote, nothing is
/// asked; a remote that cannot be asked, or publishes nothing, is left to the publish, as before.
fn join_published(
    ws: &Workspace,
    remote: Option<&OsStr>,
    offline: bool,
) -> Result<Published, Failure> {
    if offline || view(ws)?.prefix().is_some() {
        return Ok(Published::Nothing);
    }
    let Some(remote) = ws.remote(remote)?.filter(|r| !r.is_empty()) else {
        return Ok(Published::Nothing);
    };
    // asked first, so that nothing is written here for a remote that publishes nothing
    if Link::publishes(ws, &remote)? != Some(true) {
        return Ok(Published::Nothing);
    }
    match synchronise(ws, &remote) {
        Ok(synced) => Ok(Published::Joined(Joined {
            pulled: synced.pulled,
            remote: synced.remote.to_string_lossy().into_owned(),
        })),
        // published when asked and gone since: nothing to join, unless an adoption was already
        // remembered, which the next command follows: then this one starts nothing either
        Err(error @ SyncError::NotInitialized(_)) => {
            if find_workspace(&ws.settings)?.id == ws.id {
                Ok(Published::Nothing)
            } else {
                Err(error.into())
            }
        }
        Err(error) => Err(error.into()),
    }
}

/// Writes `.beadroll.json` at the top of the checkout: the configuration it has (unknown keys
/// kept), a workspace id (this tracker's when `keep`, as when the remote already published it,
/// else minted) unless it names one, and `remote`, or the configured remote without a password:
/// none is ever written back, even a hand-set one.
fn write_tracked(ws: &Workspace, remote: Option<&OsStr>, keep: bool) -> Result<(), Failure> {
    let settings = &ws.settings;
    let top = settings
        .fact(Fact::Toplevel)?
        .checked_text(Fact::Toplevel.args())?;
    let mut config: Object = ws.config.clone();
    let workspace = match config.get("workspace") {
        Some(value) if value.truthy() => value.clone(),
        _ if keep => Value::from(ws.id.as_str()),
        _ => Value::from(uuid7(&settings.clock)),
    };
    config.insert("workspace".into(), workspace);
    if let Some(remote) = remote {
        config.insert("remote".into(), Value::from(text(remote)?));
    } else if let Some(Value::Str(remote)) = config.get("remote").filter(|r| r.truthy()) {
        let kept = without_password(OsStr::new(remote)).0;
        config.insert("remote".into(), Value::from(text(&kept)?));
    }
    let mut body = json::write(&Value::Object(config), Format::INDENTED)
        .map_err(|e| Failure::new(beadroll_core::code::Code::Validation, e.to_string()))?;
    body.push('\n');
    write_atomic(
        &Path::new(&top).join(".beadroll.json"),
        body.as_bytes(),
        false,
        &settings.failpoints,
    )?;
    Ok(())
}

/// The workspace `init` writes to, `None` for this checkout's own, and the tracker it joined.
struct Chosen {
    ws: Option<Workspace>,
    joined: Option<Joined>,
}

/// Where `init` writes: the tracker the remote already publishes ([`join_published`]), else this
/// checkout's; with `--tracked`, the one `.beadroll.json` names once written, with the joined
/// tracker's id, and origin's own URL stored as `origin` (`names_origin`).
fn workspace_for(
    ctx: &Context<'_>,
    remote: Option<&OsStr>,
    names_origin: bool,
    tracked: bool,
    offline: bool,
) -> Result<Chosen, Failure> {
    let settings = &ctx.ws.settings;
    let (ws, joined) = match join_published(ctx.ws, remote, offline)? {
        Published::Joined(joined) => (Some(find_workspace(settings)?), Some(joined)),
        Published::Nothing => (None, None),
    };
    if !tracked {
        return Ok(Chosen { ws, joined });
    }
    let base = ws.as_ref().unwrap_or(ctx.ws);
    // a URL-derived id named in a tracked file would stop matching when the URL changed, and the
    // file would then be refused as another repository's id: such a tracker stays URL-derived
    if joined.is_some() && derived_shaped(&base.id) {
        return Err(Failure::new(
            Code::Config,
            format!(
                "this repository's tracker is URL-derived (workspace {}), and it is now here; a \
                 .beadroll.json naming that id would stop working when the URL changes, so none \
                 was written. Keep the URL-derived setup; a .beadroll.json without a workspace \
                 id can still hold \"where\": false or another remote",
                base.id
            ),
        ));
    }
    let origin = OsString::from("origin");
    let stored = if names_origin {
        Some(origin.as_os_str())
    } else {
        remote
    };
    write_tracked(base, stored, joined.is_some())?;
    Ok(Chosen {
        ws: Some(find_workspace(settings)?),
        joined,
    })
}

/// `init`'s `--remote` as it is stored ([`remote_argument`]), with whether it is origin's own URL,
/// which a tracked file stores by name so that it follows origin when it moves; a dropped password
/// is said in `notes`.
fn given_remote(
    ctx: &Context<'_>,
    args: &InitArgs,
    notes: &mut Vec<String>,
) -> Result<(Option<OsString>, bool), Failure> {
    let Some(remote) = args.remote.clone().filter(|r| !r.is_empty()) else {
        return Ok((None, false));
    };
    let names_origin = ctx.ws.is_origin(&without_password(&remote).0);
    let (remote, had_password) = remote_argument(&ctx.ws.settings, &remote, args.tracked)?;
    if had_password {
        notes.push(
            "the password was dropped from the remote URL: bead never stores one; use a git \
             credential helper"
                .to_owned(),
        );
    }
    Ok((Some(remote).filter(|r| !r.is_empty()), names_origin))
}

/// Creates the tracker here: the `init` event (once), an optional import, the remote recorded,
/// and publishes at once unless `--offline`. A tracker the remote already publishes is joined
/// instead of a second one started ([`join_published`]).
pub(crate) fn init(ctx: &Context<'_>, args: InitArgs) -> Result<Outcome, Failure> {
    let mut notes = Vec::new();
    let (remote_given, names_origin) = given_remote(ctx, &args, &mut notes)?;
    let Chosen { ws: chosen, joined } = workspace_for(
        ctx,
        remote_given.as_deref(),
        names_origin,
        args.tracked,
        args.offline,
    )?;
    let ws: &Workspace = chosen.as_ref().unwrap_or(ctx.ws);
    let prefix_flag = args.prefix.as_deref().map(text).transpose()?;
    let (prefix, imported) = writing(ws, ctx.actor, |tx| -> Result<_, Failure> {
        if tx.state()?.prefix().is_none() {
            let name = match &ws.name {
                Some(name) if !name.is_empty() => Some(
                    name.to_str()
                        .ok_or_else(|| {
                            beadroll_sync::Error::Validation("text is not valid UTF-8".to_owned())
                        })?
                        .to_owned(),
                ),
                _ => None,
            };
            let prefix = match prefix_flag.as_ref().filter(|p| !p.is_empty()) {
                Some(prefix) => prefix.clone(),
                None => default_prefix(name.as_deref().unwrap_or("bead")),
            };
            let mut data = Object::new();
            data.insert("prefix".into(), Value::from(prefix.as_str()));
            data.insert("name".into(), Value::from(name.unwrap_or(prefix)));
            tx.emit("init", None, data, None)?;
        }
        let imported = match args.import_path.as_ref().filter(|p| !p.is_empty()) {
            Some(path) => import_records(tx, path)?,
            None => 0,
        };
        let prefix = tx.state()?.prefix().cloned().unwrap_or(Value::Null);
        Ok((prefix, imported))
    })?;
    // `init --tracked --remote` is the deliberate switch: record it, so the tracked remote is the
    // one in use.
    let recorded = if args.tracked {
        match &remote_given {
            Some(remote) => Some(ws.resolve_remote(remote, false)?),
            None => None,
        }
    } else {
        remote_given.clone()
    };
    let mut values = Vec::new();
    if let Some(name) = &ws.name {
        values.push(("name", Value::from(name.to_string_lossy().into_owned())));
    }
    if let Some(remote) = &recorded {
        values.push(("remote", Value::from(remote.to_string_lossy().into_owned())));
    }
    ws.save_meta(&values)?;
    if ws.wrote() && !args.offline {
        // the tracker exists for the other machines now, not at a later sync
        if let Some(warning) = publish_soon(ws, args.offline, remote_given.as_deref()) {
            notes.push(warning);
        }
    }
    let mut result = Object::new();
    result.insert("workspace".into(), Value::from(ws.id.as_str()));
    result.insert("prefix".into(), prefix.clone());
    result.insert("path".into(), Value::from(ws.dir.display().to_string()));
    result.insert("imported".into(), Value::from(imported as i64));
    result.insert("tracked_config".into(), Value::Bool(!ws.config.is_empty()));
    result.insert("joined".into(), Value::Bool(joined.is_some()));
    let mut lines = Vec::new();
    if let Some(Joined { pulled, remote }) = &joined {
        lines.push(format!(
            "{remote} already publishes this repository's tracker: joined it (pulled {pulled} \
             event(s)), no second one started"
        ));
    }
    lines.push(format!(
        "Tracker {} at {}{}",
        repr(&prefix),
        ws.dir.display(),
        if imported > 0 {
            format!(" ({imported} issues imported)")
        } else {
            String::new()
        }
    ));
    if args.tracked {
        lines.push(
            "Wrote .beadroll.json: commit it so every checkout and machine uses this workspace"
                .to_owned(),
        );
    }
    lines.extend(notes);
    let mut outcome = Outcome::new(result, lines);
    if let Some(chosen) = &chosen {
        // written through another workspace than the one the command was given: its own count
        super::note_unpublished(chosen, &mut outcome.result, &mut outcome.lines)?;
        outcome.settled = true;
    }
    Ok(outcome)
}
