//! Which tracker a checkout belongs to (Python's `find_workspace`): from the repository, never from
//! the checkout's path.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use beadroll_core::json::{Object, Value};
use beadroll_core::text::repr_str;
use beadroll_git::{Fact, Output, is_local_path, normalize_remote, realpath};
use sha2::{Digest as _, Sha256};

use crate::error::Error;
use crate::fs::{Unreadable, json_text, read_json_text};
use crate::settings::{Settings, pathlib, pathlib_join};
use crate::workspace::{Workspace, configured_url, listed_url};

/// The tracked configuration's keys bead reads, and the JSON type each must have.
const CONFIG_KEYS: [(&str, &str); 4] = [
    ("workspace", "str"),
    ("ref", "str"),
    ("remote", "str"),
    ("where", "bool"),
];

/// The tracked configuration, or a `CONFIG` error naming what is wrong with it. Unknown keys are
/// left for newer versions to understand.
pub fn checked_config(config: Value, path: &str) -> Result<Object, Error> {
    let Value::Object(config) = config else {
        return Err(Error::Config(format!("{path} must hold a JSON object")));
    };
    for (key, kind) in CONFIG_KEYS {
        let fits = matches!(
            (config.get(key), kind),
            (None, _) | (Some(Value::Str(_)), "str") | (Some(Value::Bool(_)), "bool")
        );
        if !fits {
            return Err(Error::Config(format!(
                "{path}: {} must be a {kind}",
                repr_str(key)
            )));
        }
        if kind == "str" && matches!(config.get(key), Some(Value::Str(s)) if s.is_empty()) {
            // an empty value would silently mean "not set"
            return Err(Error::Config(format!(
                "{path}: {} must not be empty",
                repr_str(key)
            )));
        }
    }
    Ok(config)
}

/// The committed `.beadroll.json`, for a checkout whose tree has none (a worktree on a branch cut
/// before it was committed, a bisect, a bare clone): HEAD's copy first, then the default branch's.
/// One git call.
fn committed_config(settings: &Settings) -> Result<Object, Error> {
    let refs = ["HEAD", "refs/remotes/origin/HEAD"];
    let input: String = refs
        .iter()
        .map(|r| format!("{r}:.beadroll.json\n"))
        .collect();
    let proc = settings
        .git
        .git(["cat-file", "--batch"])
        .input(input.into_bytes())
        .run()?;
    let mut out: &[u8] = if proc.success() { &proc.stdout } else { b"" };
    for name in refs {
        let (header, rest) = match out.iter().position(|b| *b == b'\n') {
            Some(at) => (&out[..at], &out[at + 1..]),
            None => (out, &b""[..]),
        };
        out = rest;
        let fields: Vec<&[u8]> = header
            .split(|b| b.is_ascii_whitespace())
            .filter(|f| !f.is_empty())
            .collect();
        let size = match fields.as_slice() {
            [_, _, size] if !size.is_empty() && size.iter().all(u8::is_ascii_digit) => {
                std::str::from_utf8(size)
                    .ok()
                    .and_then(|s| s.parse::<usize>().ok())
            }
            _ => None,
        };
        let Some(size) = size else {
            continue; // "<name> missing": not in that commit, or no such ref
        };
        let body = out.get(..size).unwrap_or(out);
        out = out.get(size + 1..).unwrap_or_default();
        if fields.get(1) == Some(&&b"blob"[..]) {
            let origin = format!("{name}:.beadroll.json");
            let config = json_text(body)
                .map_err(|_| Error::Config(format!("{origin} is not valid JSON")))?;
            return checked_config(config, &origin);
        }
    }
    Ok(Object::new())
}

/// Why this checkout's `.beadroll.json` does not reach the other checkouts and machines, if it does
/// not: it is not committed (untracked, ignored or only staged), its content is not its commit's
/// (whatever `--skip-worktree` or `--assume-unchanged` hide from `git status`), or origin's default
/// branch, which a clone and a worktree on an older branch read it from ([`committed_config`]),
/// lacks it or holds another; with an `origin` but no `refs/remotes/origin/HEAD` to tell its
/// default branch by, that is said too. The same goes for HEAD's copy where the tree has none
/// (deleted, or left out of a sparse checkout), which `ws` then uses. `None` when the copy in use is
/// on the default branch unchanged, or when `ws` uses none: then git is not asked. One git call,
/// two when the file is not committed (to say why).
pub fn tracked_config_warning(ws: &Workspace) -> Result<Option<String>, Error> {
    let settings = &ws.settings;
    let top = settings.fact(Fact::Toplevel)?;
    if !top.success() {
        return Ok(None);
    }
    let root = pathlib(OsStr::from_bytes(crate::strip_ascii(&top.stdout)));
    let here = std::fs::read(root.join(".beadroll.json")).ok();
    if here.is_none() && ws.config.is_empty() {
        return Ok(None);
    }
    let input =
        "HEAD:.beadroll.json\nrefs/remotes/origin/HEAD\nrefs/remotes/origin/HEAD:.beadroll.json\n";
    let found = settings
        .git
        .git(["cat-file", "--batch"])
        .input(input.as_bytes().to_vec())
        .run()?;
    let objects = batch_objects(if found.success() { &found.stdout } else { b"" }, 3);
    let committed = match (objects[0].as_ref(), &here) {
        // neither here nor at HEAD: the default branch's copy, which every clone reads, or none
        (None, None) => return Ok(None),
        (None, Some(_)) => return not_committed(settings).map(Some),
        (Some((_, content)), Some(here)) if !same_settings(content, here) => {
            return Ok(Some(
                ".beadroll.json differs from its committed copy, which the other checkouts and \
                 machines use: commit the change"
                    .to_owned(),
            ));
        }
        // the tree's copy is the commit's, or the tree has none and HEAD's is the one in use
        (Some((committed, _)), _) => committed,
    };
    Ok(match (&objects[1], &objects[2]) {
        (None, _) if ws.origin.is_some() => Some(
            ".beadroll.json is committed, but this checkout cannot tell whether origin's default \
             branch, which clones read it from, has it (no refs/remotes/origin/HEAD): `git remote \
             set-head origin --auto` lets bead check"
                .to_owned(),
        ),
        (Some(_), None) => Some(
            ".beadroll.json is committed here but not on origin's default branch: clones and \
             worktrees on other branches do not see it until it is merged there"
                .to_owned(),
        ),
        (Some(_), Some((there, _))) if there != committed => Some(
            ".beadroll.json differs from the copy on origin's default branch, which clones and \
             worktrees on other branches read: merge it there"
                .to_owned(),
        ),
        _ => None,
    })
}

/// Why the tree's `.beadroll.json`, which HEAD lacks, is not committed, as git's status says it:
/// ignored, only added, or neither.
fn not_committed(settings: &Settings) -> Result<String, Error> {
    let unseen = "other worktrees, clones and machines of this repository do not see it, and \
                  use the workspace derived from the origin URL instead";
    let status = settings
        .git
        .git([
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--ignored",
            "--",
            ":/.beadroll.json",
        ])
        .run()?;
    let code: &[u8] = status.stdout.get(..2).unwrap_or_default();
    Ok(match code {
        b"!!" => format!(
            ".beadroll.json is ignored by git, so it is never committed: {unseen}; commit it"
        ),
        [b'A', _] => format!(".beadroll.json is added but not committed: {unseen}; commit it"),
        _ => format!(".beadroll.json is not committed: {unseen}; commit it"),
    })
}

/// Whether two texts of `.beadroll.json` hold the same settings: compared as JSON, so that line
/// endings git converts on checkout (`core.autocrlf`, `eol`), or layout alone, make no difference;
/// a text that is not JSON is compared as it is.
fn same_settings(committed: &[u8], here: &[u8]) -> bool {
    match (json_text(committed), json_text(here)) {
        (Ok(committed), Ok(here)) => committed == here,
        _ => committed == here,
    }
}

/// The `count` objects `git cat-file --batch` printed, in order: each one's id and content, or
/// `None` where it said `missing`.
fn batch_objects(mut out: &[u8], count: usize) -> Vec<Option<(Vec<u8>, Vec<u8>)>> {
    let mut objects = Vec::with_capacity(count);
    for _ in 0..count {
        let (header, rest) = match out.iter().position(|b| *b == b'\n') {
            Some(at) => (&out[..at], &out[at + 1..]),
            None => (out, &b""[..]),
        };
        out = rest;
        let fields: Vec<&[u8]> = header.split(|b| *b == b' ').collect();
        let size = match fields.as_slice() {
            [_, _, size] => std::str::from_utf8(size)
                .ok()
                .and_then(|s| s.parse::<usize>().ok()),
            _ => None,
        };
        let Some(size) = size else {
            objects.push(None);
            continue;
        };
        let content = out.get(..size).unwrap_or(out).to_vec();
        out = out.get(size + 1..).unwrap_or_default();
        objects.push(Some((fields[0].to_vec(), content)));
    }
    objects
}

/// The other workspaces of this store whose `meta.json` names `name`, by id: where a repository
/// that moved left its tracker, when nothing is published at its new URL.
pub fn workspaces_named(
    settings: &Settings,
    name: &OsStr,
    except: &str,
) -> Result<Vec<String>, Error> {
    let Ok(entries) = std::fs::read_dir(settings.home()?) else {
        return Ok(Vec::new());
    };
    let wanted = name.to_string_lossy();
    let mut found: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| (entry.file_name().to_string_lossy().into_owned(), entry.path()))
        .filter(|(id, _)| id != except)
        .filter(|(_, path)| {
            matches!(read_json_text(&path.join("meta.json")),
                Ok(Value::Object(meta)) if matches!(meta.get("name"), Some(Value::Str(n)) if *n == wanted))
        })
        .map(|(id, _)| id)
        .collect();
    found.sort();
    Ok(found)
}

/// URL-derived ids whose repository's tracker was found under another id (`adopt_published`).
pub fn moved_workspaces(settings: &Settings) -> Result<Object, Error> {
    let path = settings.home()?.join("moved.json");
    Ok(match read_json_text(&path) {
        Ok(Value::Object(moved)) => moved
            .into_iter()
            .filter(|(_, id)| matches!(id, Value::Str(_)))
            .collect(),
        _ => Object::new(),
    })
}

/// The checkout's origin, from what `git config --get-regexp ^remote\.origin\.(push)?url$`
/// printed there (`asked`): its [`configured_url`] as configured, the URL bead gives git to reach
/// origin as `git push origin` does (a relative path, or a spelling a rule rewrites, left to git),
/// and whether that is its own push URL.
fn origin(asked: Result<Output, beadroll_git::Error>) -> Result<(OsString, bool), Error> {
    let proc = asked?;
    if !proc.success() {
        return Ok((OsString::new(), false));
    }
    let push = listed_url(&proc.stdout, b"origin", b"pushurl").is_some();
    let url = configured_url(&proc.stdout, b"origin");
    Ok((OsString::from_vec(url), push))
}

/// `origin` as the URL-derived id reads it: a relative path from the top of the checkout.
fn anchored(origin: &OsStr, root: Option<&Path>) -> OsString {
    match root {
        Some(root) if is_local_path(origin) && !origin.as_bytes().starts_with(b"/") => {
            pathlib_join(root, origin).into_os_string()
        }
        _ => origin.to_owned(),
    }
}

/// Identifies the tracker from the repository, never from the checkout path. A tracked
/// `.beadroll.json` names the workspace explicitly; without one, the id is derived from the origin
/// URL, so every worktree and clone agrees; without an origin, it is local to the repository.
pub fn find_workspace(settings: &Arc<Settings>) -> Result<Workspace, Error> {
    // origin's URLs, asked each time as Python asks them; the first time, beside the top of the
    // checkout, which does not wait for them (their answer, or failure, is read where Python asks)
    let origin_urls = settings.checkout.origin_urls(&settings.git);
    let top = settings.fact(Fact::Toplevel)?;
    let root: Option<PathBuf> = top.success().then(|| {
        let raw = crate::strip_ascii(&top.stdout);
        pathlib(OsStr::from_bytes(raw))
    });
    let tracked_file = root.as_ref().map(|root| root.join(".beadroll.json"));
    if let Some(file) = &tracked_file
        && file.is_symlink()
        && !file.exists()
    {
        return Err(Error::Config(format!(
            "{} is a symbolic link that does not resolve to a file",
            file.display()
        )));
    }
    let config = match &tracked_file {
        Some(file) if file.exists() => {
            let shown = file.display().to_string();
            let config = match read_json_text(file) {
                Ok(value) => value,
                Err(Unreadable::Missing | Unreadable::Other(_)) => {
                    return Err(Error::Config(format!("{shown} is not valid JSON")));
                }
            };
            checked_config(config, &shown)?
        }
        _ => committed_config(settings)?,
    };
    let (origin, origin_push) = origin(origin_urls)?;
    let cwd = &settings.cwd;
    let normalized =
        (!origin.is_empty()).then(|| normalize_remote(&anchored(&origin, root.as_deref()), cwd));
    let name: Option<OsString> = match &normalized {
        Some(normalized) => {
            let bytes = normalized.as_bytes();
            let last = bytes
                .iter()
                .rposition(|b| *b == b'/')
                .map_or(bytes, |at| &bytes[at + 1..]);
            Some(OsString::from(String::from_utf8_lossy(last).into_owned()))
        }
        None => root
            .as_ref()
            .and_then(|root| root.file_name())
            .map(OsStr::to_owned),
    };
    let derived = normalized
        .as_ref()
        .map(|normalized| hex16(&Sha256::digest(normalized.as_bytes())));
    let overridden = settings.var("BEAD_WORKSPACE").map(OsStr::to_owned);
    let tracked_id = match config.get("workspace") {
        Some(Value::Str(id)) if !id.is_empty() => Some(id.clone()),
        _ => None,
    };
    if overridden.is_none()
        && let Some(tracked) = &tracked_id
        && Some(tracked) != derived.as_ref()
        && derived_shaped(tracked)
    {
        return Err(Error::Config(format!(
            ".beadroll.json names workspace {tracked}, an id another repository derives from its \
             own origin; a tracked workspace id is one `bead init --tracked` mints"
        )));
    }
    let mut id: Option<String> = match &overridden {
        Some(value) => Some(value.to_string_lossy().into_owned()),
        None => tracked_id.clone(),
    };
    if id.is_none()
        && let Some(derived) = &derived
    {
        let moved = moved_workspaces(settings)?;
        id = Some(match moved.get(derived) {
            Some(Value::Str(found)) => found.clone(),
            _ => derived.clone(),
        });
    }
    let id = match id {
        Some(id) => id,
        None => local_id(settings)?,
    };
    let mut ws = Workspace::new(
        Arc::clone(settings),
        &id,
        name,
        (!origin.is_empty()).then_some(origin),
        config,
    )?;
    ws.origin_push = origin_push;
    ws.derived = if overridden.is_none() && tracked_id.is_none() {
        derived
    } else {
        None
    };
    Ok(ws)
}

/// The id of a repository without an origin: a hash of its common directory, symlinks resolved.
fn local_id(settings: &Settings) -> Result<String, Error> {
    let common = settings.fact(Fact::CommonDir)?;
    if !common.success() {
        // git's own reason, in the user's language: no repository here, or one git refuses to use
        // (dubious ownership, with the safe.directory command that fixes it). One line.
        let said = String::from_utf8_lossy(&common.stderr);
        let said: Vec<&str> = said
            .split(beadroll_core::text::is_space)
            .filter(|word| !word.is_empty())
            .collect();
        return Err(Error::NoWorkspace(if said.is_empty() {
            "not inside a git repository (or set BEAD_WORKSPACE)".to_owned()
        } else {
            format!(
                "no usable git repository here: {} (outside a repository, set BEAD_WORKSPACE)",
                said.join(" ")
            )
        }));
    }
    let dir = OsStr::from_bytes(crate::strip_ascii(&common.stdout));
    let real = realpath(dir, &settings.cwd);
    let digest = Sha256::digest(real.as_os_str().as_bytes());
    Ok(format!("local-{}", &hex16(&digest)[..12]))
}

/// `[0-9a-f]{16}|local-[0-9a-f]{12}`, whole: the shape of an id bead derives itself.
pub fn derived_shaped(id: &str) -> bool {
    let hex = |s: &str, n: usize| {
        s.len() == n && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    };
    hex(id, 16) || id.strip_prefix("local-").is_some_and(|rest| hex(rest, 12))
}

/// The first 16 hex digits of a digest.
fn hex16(digest: &[u8]) -> String {
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracked_configuration_is_checked_by_type_and_emptiness() {
        let parse = |text: &str| json_text(text.as_bytes()).unwrap();
        assert!(checked_config(parse(r#"{"workspace": "w", "x": [1]}"#), "p").is_ok());
        let error = |text: &str| checked_config(parse(text), "p").unwrap_err().to_string();
        assert_eq!(error("[]"), "p must hold a JSON object");
        assert_eq!(error(r#"{"ref": 1}"#), "p: 'ref' must be a str");
        assert_eq!(error(r#"{"where": "yes"}"#), "p: 'where' must be a bool");
        assert_eq!(error(r#"{"remote": ""}"#), "p: 'remote' must not be empty");
        assert_eq!(
            error(r#"{"workspace": true}"#),
            "p: 'workspace' must be a str"
        );
    }

    #[test]
    fn derived_ids_are_known_by_their_shape() {
        assert!(derived_shaped("0123456789abcdef") && derived_shaped("local-0123456789ab"));
        assert!(!derived_shaped("0123456789ABCDEF") && !derived_shaped("local-x"));
    }
}
