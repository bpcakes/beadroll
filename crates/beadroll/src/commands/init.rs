//! `bead init`: the tracker for this repository, optionally with a tracked `.beadroll.json` and an
//! imported Beads export.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use beadroll_core::json::{self, Format, Object, Value};
use beadroll_core::text::repr;
use beadroll_git::{exists, realpath, without_password};
use beadroll_store::{Settings, Workspace, find_workspace, uuid7, write_atomic};
use beadroll_sync::{publish_soon, writing};

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
        let top = git.git(["rev-parse", "--show-toplevel"]).run()?;
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

/// Writes `.beadroll.json` at the top of the checkout: the configuration it has (unknown keys
/// kept), a workspace id minted unless it names one, and `remote`, or the configured remote
/// without a password: none is ever written back, even a hand-set one.
fn write_tracked(ws: &Workspace, remote: Option<&OsStr>) -> Result<(), Failure> {
    let settings = &ws.settings;
    let top = settings.git.git(["rev-parse", "--show-toplevel"]).text()?;
    let mut config: Object = ws.config.clone();
    let workspace = match config.get("workspace") {
        Some(value) if value.truthy() => value.clone(),
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
    )?;
    Ok(())
}

/// Creates the tracker here: the `init` event (once), an optional import, the remote recorded,
/// and publishes at once unless `--offline`.
pub(crate) fn init(ctx: &Context<'_>, mut args: InitArgs) -> Result<Outcome, Failure> {
    let settings = Arc::clone(&ctx.ws.settings);
    let mut notes = Vec::new();
    if let Some(remote) = args.remote.clone().filter(|r| !r.is_empty()) {
        let (remote, had_password) = remote_argument(&settings, &remote, args.tracked)?;
        if had_password {
            notes.push(
                "the password was dropped from the remote URL: bead never stores one; use a git \
                 credential helper"
                    .to_owned(),
            );
        }
        args.remote = Some(remote);
    }
    let remote_given = args.remote.clone().filter(|r| !r.is_empty());
    let tracked_ws: Option<Workspace> = if args.tracked {
        write_tracked(ctx.ws, remote_given.as_deref())?;
        Some(find_workspace(&settings)?)
    } else {
        None
    };
    let ws: &Workspace = tracked_ws.as_ref().unwrap_or(ctx.ws);
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
        remote_given
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
        if let Some(warning) = publish_soon(ws, args.offline, args.remote.as_deref()) {
            notes.push(warning);
        }
    }
    let mut result = Object::new();
    result.insert("workspace".into(), Value::from(ws.id.as_str()));
    result.insert("prefix".into(), prefix.clone());
    result.insert("path".into(), Value::from(ws.dir.display().to_string()));
    result.insert("imported".into(), Value::from(imported as i64));
    result.insert("tracked_config".into(), Value::Bool(!ws.config.is_empty()));
    let mut lines = vec![format!(
        "Tracker {} at {}{}",
        repr(&prefix),
        ws.dir.display(),
        if imported > 0 {
            format!(" ({imported} issues imported)")
        } else {
            String::new()
        }
    )];
    if args.tracked {
        lines.push(
            "Wrote .beadroll.json: commit it so every checkout and machine uses this workspace"
                .to_owned(),
        );
    }
    lines.extend(notes);
    Ok(Outcome::new(result, lines))
}
