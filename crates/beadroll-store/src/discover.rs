//! Which tracker a checkout belongs to (Python's `find_workspace`): from the repository, never from
//! the checkout's path.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use beadroll_core::json::{Object, Value};
use beadroll_core::text::repr_str;
use beadroll_git::{is_local_path, normalize_remote, realpath};
use sha2::{Digest as _, Sha256};

use crate::error::Error;
use crate::fs::{Unreadable, json_text, read_json_text};
use crate::settings::{Settings, pathlib, pathlib_join};
use crate::workspace::Workspace;

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

/// The checkout's origin: where `git push origin` goes, its first push URL, else its first URL (the
/// one git fetches from), as configured (`insteadOf` stays git's, not the id's). A relative path is
/// read from the top of the checkout.
fn origin(settings: &Settings, root: Option<&Path>) -> Result<OsString, Error> {
    let proc = settings
        .git
        .git(["config", "--get-regexp", r"^remote\.origin\.(push)?url$"])
        .run()?;
    let mut found: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    if proc.success() {
        for line in split_lines(&proc.stdout) {
            let (key, url) = match line.iter().position(|b| *b == b' ') {
                Some(at) => (&line[..at], &line[at + 1..]),
                None => (line, &b""[..]),
            };
            found.push((key.to_vec(), url.to_vec()));
        }
    }
    let mut origin = Vec::new();
    'keys: for wanted in ["remote.origin.pushurl", "remote.origin.url"] {
        for (key, url) in &found {
            let url = beadroll_git::py_strip(url);
            if key.as_slice() == wanted.as_bytes() && !url.is_empty() {
                origin = url.to_vec();
                break 'keys;
            }
        }
    }
    let origin = OsString::from_vec(origin);
    if let Some(root) = root
        && !origin.is_empty()
        && is_local_path(&origin)
        && !origin.as_bytes().starts_with(b"/")
    {
        return Ok(pathlib_join(root, &origin).into_os_string());
    }
    Ok(origin)
}

/// Python's `str.splitlines()` over text decoded with `surrogateescape`: the line boundaries are
/// ASCII or multi-byte characters that never appear inside another character's encoding.
fn split_lines(data: &[u8]) -> Vec<&[u8]> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut at = 0;
    while at < data.len() {
        let width = match &data[at..] {
            [b'\r', b'\n', ..] => 2,
            [b'\n' | b'\r' | 0x0b | 0x0c | 0x1c | 0x1d | 0x1e, ..] => 1,
            [0xc2, 0x85, ..] => 2,
            [0xe2, 0x80, 0xa8 | 0xa9, ..] => 3,
            _ => 0,
        };
        if width == 0 {
            at += 1;
            continue;
        }
        lines.push(&data[start..at]);
        at += width;
        start = at;
    }
    if start < data.len() {
        lines.push(&data[start..]);
    }
    lines
}

/// Identifies the tracker from the repository, never from the checkout path. A tracked
/// `.beadroll.json` names the workspace explicitly; without one, the id is derived from the origin
/// URL, so every worktree and clone agrees; without an origin, it is local to the repository.
pub fn find_workspace(settings: &Arc<Settings>) -> Result<Workspace, Error> {
    let git = &settings.git;
    let top = git.git(["rev-parse", "--show-toplevel"]).run()?;
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
    let origin = origin(settings, root.as_deref())?;
    let cwd = &settings.cwd;
    let normalized = (!origin.is_empty()).then(|| normalize_remote(&origin, cwd));
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
    ws.derived = if overridden.is_none() && tracked_id.is_none() {
        derived
    } else {
        None
    };
    Ok(ws)
}

/// The id of a repository without an origin: a hash of its common directory, symlinks resolved.
fn local_id(settings: &Settings) -> Result<String, Error> {
    let common = settings
        .git
        .git(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .run()?;
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
fn derived_shaped(id: &str) -> bool {
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
    fn lines_split_as_python_splits_decoded_text() {
        assert_eq!(
            split_lines(b"a b\r\nc\xc2\x85d\xffe\n"),
            [&b"a b"[..], b"c", b"d\xffe"]
        );
        assert!(derived_shaped("0123456789abcdef") && derived_shaped("local-0123456789ab"));
        assert!(!derived_shaped("0123456789ABCDEF") && !derived_shaped("local-x"));
    }
}
