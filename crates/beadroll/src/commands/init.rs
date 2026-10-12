//! `bead init`: the tracker for this repository, optionally with a tracked `.beadroll.json` and an
//! imported Beads export.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
use std::path::{Path, PathBuf};

use beadroll_core::code::Code;
use beadroll_core::json::{self, Format, Object, Value};
use beadroll_core::text::repr;
use beadroll_git::{Fact, is_local_path, realpath, without_password};
use beadroll_store::{
    Endpoint, Workspace, derived_shaped, find_workspace, remembered, uuid7, view_outline,
    write_atomic,
};
use beadroll_sync::{
    Error as SyncError, Link, publish_soon, publish_soon_to, synchronise, writing,
};

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

/// The relative path to `path` from `start`, both absolute.
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

/// How `init --tracked` stores the `--remote` it was given, typed as `typed` and leading to `live`:
/// as typed when it is a configured remote's name, a URL, an absolute path or a path git rewrites;
/// any other relative path, typed in a subdirectory or not and whether it exists or not, relative
/// to the top of the checkout, which a stored one is read from.
fn stored_remote(ws: &Workspace, typed: &OsStr, live: &Endpoint) -> Result<OsString, Failure> {
    let relative = is_local_path(typed) && !typed.as_bytes().starts_with(b"/");
    if !relative || ws.names_remote(typed)? || ws.rewrites(typed)? {
        return Ok(typed.to_owned());
    }
    let settings = &ws.settings;
    let cwd = &settings.cwd;
    let top = settings.fact(Fact::Toplevel)?;
    // the top as a stored path is read from: git's line, stripped, then a slash
    let base = top.success().then(|| {
        let mut base = beadroll_core::text::strip_bytes(&top.stdout).to_vec();
        if !base.ends_with(b"/") {
            base.push(b'/');
        }
        base
    });
    if ws.rewrites(&live.url)? {
        // where it leads, as written, since git rewrites it so: the very spelling that reads back
        // as it from the top of the checkout (`.`, `..` and symbolic links kept), else whole
        let written = base
            .as_ref()
            .and_then(|base| live.url.as_bytes().strip_prefix(base.as_slice()));
        return Ok(written.map_or_else(
            || live.url.clone(),
            |rest| OsStr::from_bytes(rest).to_owned(),
        ));
    }
    let target = realpath(&live.url, cwd);
    let Some(base) = base else {
        return Ok(target.into_os_string());
    };
    let top_dir = realpath(OsStr::from_bytes(&base), cwd);
    Ok(relpath(&target, &top_dir).into_os_string())
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
    remote: Option<&Endpoint>,
    offline: bool,
) -> Result<Published, Failure> {
    if offline || view_outline(ws)?.prefix().is_some() {
        return Ok(Published::Nothing);
    }
    let remote = match remote {
        Some(remote) => Some(remote.clone()),
        None => ws.endpoint(None)?,
    };
    let Some(remote) = remote.filter(|remote| !remote.url.is_empty()) else {
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
/// kept), a workspace id (this tracker's when `keep`, as when it is already here or the remote
/// published it, else minted) unless it names one, and `remote`, or the configured remote without a password:
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
    let mut body = json::write(&Value::from(config), Format::INDENTED)
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

/// Where `init` writes: the tracker the remote already publishes ([`join_published`], reached
/// where `live` leads), else this checkout's; with `--tracked`, the one `.beadroll.json` names once
/// written, with the joined tracker's id, and `remote` as it is stored, origin's own URL as
/// `origin` (`names_origin`).
fn workspace_for(
    ctx: &Context<'_>,
    remote: Option<&OsStr>,
    live: Option<&Endpoint>,
    names_origin: bool,
    tracked: bool,
    offline: bool,
) -> Result<Chosen, Failure> {
    let settings = &ctx.ws.settings;
    let (ws, joined) = match join_published(ctx.ws, live, offline)? {
        Published::Joined(joined) => (Some(find_workspace(settings)?), Some(joined)),
        Published::Nothing => (None, None),
    };
    if !tracked {
        return Ok(Chosen { ws, joined });
    }
    let base = ws.as_ref().unwrap_or(ctx.ws);
    // the tracker this checkout has, joined just now or here already (an earlier init may have
    // adopted it): the file names it, since a new id would start a second tracker beside it
    let has_tracker = joined.is_some() || view_outline(base)?.prefix().is_some();
    // a URL-derived id named in a tracked file would stop matching when the URL changed, and the
    // file would then be refused as another repository's id: such a tracker stays URL-derived
    if has_tracker && derived_shaped(&base.id) {
        return Err(Failure::new(
            Code::Config,
            format!(
                "this repository's tracker is URL-derived (workspace {}), and it is here; a \
                 .beadroll.json naming that id would stop working when the URL changes, and one \
                 naming a new id would start a second tracker, so none was written. Keep the \
                 URL-derived setup; a .beadroll.json without a workspace id can still hold \
                 \"where\": false or another remote",
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
    write_tracked(base, stored, has_tracker)?;
    Ok(Chosen {
        ws: Some(find_workspace(settings)?),
        joined,
    })
}

/// `init`'s `--remote`: where it leads, read from where it was typed ([`Workspace::resolve_endpoint`]),
/// and, with `--tracked`, how the file stores it ([`stored_remote`]), else as typed; with whether it
/// is origin, by its name or its own URL, which a tracked file stores by name so that it follows
/// origin when it moves. A dropped password is said in `notes`.
fn given_remote(
    ctx: &Context<'_>,
    args: &InitArgs,
    notes: &mut Vec<String>,
) -> Result<Option<Given>, Failure> {
    let Some(remote) = args.remote.clone().filter(|r| !r.is_empty()) else {
        return Ok(None);
    };
    let (typed, had_password) = without_password(&remote);
    if had_password {
        notes.push(
            "the password was dropped from the remote URL: bead never stores one; use a git \
             credential helper"
                .to_owned(),
        );
    }
    // a configured remote's name wins over a same-named path, origin's included; origin's own
    // URL is origin, reached as origin is (its own push URL included), as every command after
    // reaches it, whether the file stores it by name or no file is written
    let live = ctx.ws.resolve_endpoint(&typed, true)?;
    let names_origin = live.name.as_deref() == Some(OsStr::new("origin"));
    let stored = if args.tracked {
        stored_remote(ctx.ws, &typed, &live)?
    } else {
        typed
    };
    Ok(Some(Given {
        names_origin,
        stored,
        live,
    }))
}

/// The `--remote` `init` was given ([`given_remote`]).
struct Given {
    /// How the tracked file stores it.
    stored: OsString,
    /// Where it leads.
    live: Endpoint,
    /// Whether it is origin, by its name or its own URL.
    names_origin: bool,
}

/// Publishes to the chosen endpoint and reports either its destination or the existing warning.
fn publication_note(ws: &Workspace, live: Option<&Endpoint>) -> Result<Option<String>, Failure> {
    let warning = match live {
        Some(remote) => publish_soon_to(ws, remote),
        None => publish_soon(ws, false, None),
    };
    if warning.is_some() {
        return Ok(warning);
    }
    let remote = match live {
        Some(remote) => Some(remote.clone()),
        None => ws.endpoint(None)?,
    };
    // No warning also means there was no remote: only acknowledge a real publication.
    let Some(remote) = remote.filter(|remote| !remote.url.is_empty()) else {
        return Ok(None);
    };
    Ok(Some(format!(
        "Published to {} ({})",
        remote.url.to_string_lossy(),
        ws.ref_name()?
    )))
}

/// Creates the tracker here: the `init` event (once), an optional import, the remote recorded,
/// and publishes at once unless `--offline`. A tracker the remote already publishes is joined
/// instead of a second one started ([`join_published`]).
pub(crate) fn init(ctx: &Context<'_>, args: InitArgs) -> Result<Outcome, Failure> {
    let mut notes = Vec::new();
    let given = given_remote(ctx, &args, &mut notes)?;
    let live = given.as_ref().map(|given| given.live.clone());
    let Chosen { ws: chosen, joined } = workspace_for(
        ctx,
        given.as_ref().map(|given| given.stored.as_os_str()),
        live.as_ref(),
        given.as_ref().is_some_and(|given| given.names_origin),
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
        // read before the import, which never changes it: the state then never takes in what an
        // import emits, which would hold every imported text beside the events (roll-lqjj.9)
        let prefix = tx.state()?.prefix().cloned().unwrap_or(Value::Null);
        let imported = match args.import_path.as_ref().filter(|p| !p.is_empty()) {
            Some(path) => import_records(tx, path)?,
            None => 0,
        };
        Ok((prefix, imported))
    })?;
    // `init --tracked --remote` is the deliberate switch: record it, so the tracked remote is the
    // one in use; resolved, as a sync records it, since a name is compared by where it leads
    let mut values = Vec::new();
    if let Some(name) = &ws.name {
        values.push(("name", Value::from(name.to_string_lossy().into_owned())));
    }
    if let Some(remote) = &live {
        values.push(("remote", remembered(remote)));
    }
    ws.save_meta(&values)?;
    if ws.wrote() && !args.offline {
        // the tracker exists for the other machines now, not at a later sync
        notes.extend(publication_note(ws, live.as_ref())?);
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
    if chosen.is_some() || live.is_some() {
        // written through another workspace than the one the command was given, or published to
        // the endpoint init chose (origin's own URL reached as origin): its own count, and no
        // publish after it through the remote as typed, which may lead elsewhere
        super::note_unpublished(ws, &mut outcome.result, &mut outcome.lines)?;
        outcome.settled = true;
    }
    Ok(outcome)
}
