//! V1: push-rejection tokens stay English under translated git, allowing races to be retried. A
//! stalled receiver cannot exceed BEAD_NET_TIMEOUT, and an uncertain outcome settles later.
//! Runner timeout hygiene is tested in beadroll-git; shared helpers for the command tests live
//! here.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::Permissions;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::{Command, Stdio};

use beadroll_test_support::Json;

mod v1_push_side_translation;
mod v1_stalled_receive_timeouts;

/// An environment, as `subprocess.run(env=...)` takes one.
type Env = BTreeMap<OsString, OsString>;

/// A locale setting: the variables it sets, a later one winning over an earlier one of its name.
type Setting = Vec<(&'static str, &'static str)>;

/// `_v1_locales`: the translated settings the review ran, plus the C control. Each blanks the
/// ambient LC_ALL, LC_MESSAGES and LANGUAGE first (an empty value counts as unset), so that only
/// the named variables decide git's language.
fn locales() -> Vec<(&'static str, Setting)> {
    let blank = [("LC_ALL", ""), ("LC_MESSAGES", ""), ("LANGUAGE", "")];
    let setting = |named: &[(&'static str, &'static str)]| -> Setting {
        blank.iter().chain(named).copied().collect()
    };
    vec![
        ("C", setting(&[("LC_ALL", "C")])),
        ("LANG=de_DE.UTF-8", setting(&[("LANG", "de_DE.UTF-8")])),
        ("LC_ALL=de_DE.UTF-8", setting(&[("LC_ALL", "de_DE.UTF-8")])),
        ("LANG=fr_FR.UTF-8", setting(&[("LANG", "fr_FR.UTF-8")])),
        (
            "LANG=fr_FR.ISO8859-1",
            setting(&[("LANG", "fr_FR.ISO8859-1")]),
        ),
        (
            "LANGUAGE=de LANG=en_US.UTF-8",
            setting(&[("LANG", "en_US.UTF-8"), ("LANGUAGE", "de")]),
        ),
    ]
}

/// `env` with `setting` applied: `{**env, **loc}`.
fn applied(env: &Env, setting: &Setting) -> Env {
    let mut env = env.clone();
    for (key, value) in setting {
        env.insert(key.into(), value.into());
    }
    env
}

/// Whether `needle` occurs in `haystack`.
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// `_v1_bead_reads_rejected`: the test `Link.publish` applies to a failed push's stderr (bead,
/// `publish`): any of these tokens means 'the remote definitely did not apply it', which a
/// coordinated command retries.
fn bead_reads_rejected(stderr: &[u8]) -> bool {
    const TOKENS: [&str; 7] = [
        "[rejected]",
        "[remote rejected]",
        "non-fast-forward",
        "fetch first",
        "stale info",
        "cannot lock ref",
        "failed to update ref",
    ];
    TOKENS
        .iter()
        .any(|token| contains(stderr, token.as_bytes()))
}

/// `_v1_git_is_translated`: whether git answers in another language than English under `env`: a
/// message from outside any repository, which every git translation covers.
fn git_is_translated(env: &Env, place: &Path) -> bool {
    let outside = place.join("not-a-repo");
    if !outside.is_dir() {
        std::fs::create_dir(&outside).unwrap();
    }
    let probe = Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(&outside)
        .env_clear()
        .envs(env)
        .env("GIT_CEILING_DIRECTORIES", place)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    !probe.status.success() && !contains(&probe.stderr, b"not a git repository")
}

/// `_v1_winner_identity`: shell lines giving a hook's own commit an identity, whatever the user's
/// git config holds.
const WINNER_IDENTITY: &str = concat!(
    "export GIT_AUTHOR_NAME=winner GIT_AUTHOR_EMAIL=w@example.invalid ",
    "GIT_COMMITTER_NAME=winner GIT_COMMITTER_EMAIL=w@example.invalid\n"
);

/// `path` quoted for the shell.
fn quoted(path: &Path) -> String {
    let text = path.to_str().expect("a UTF-8 path");
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// `script` written to `path`, executable.
fn executable(path: &Path, script: &str) {
    std::fs::write(path, script).unwrap();
    std::fs::set_permissions(path, Permissions::from_mode(0o755)).unwrap();
}

/// The id of the issue a `--json` result is: `result["id"]`.
fn id(result: Option<Json>) -> String {
    let result = result.expect("bead printed a result");
    result["id"].as_str().expect("an id").to_owned()
}

/// The member `key` of `value`; absence fails the test.
fn member<'v>(value: &'v Json, key: &str) -> &'v Json {
    value
        .get(key)
        .unwrap_or_else(|| panic!("no {key:?} in {value:?}"))
}
