//! V1: push-rejection tokens stay English under translated git, allowing races to be retried. A
//! stalled receiver cannot exceed BEAD_NET_TIMEOUT, and an uncertain outcome settles later.
//! Runner timeout hygiene is tested in beadroll-git; shared helpers for the command tests live
//! here.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;
use std::process::{Command, Stdio};

use beadroll_test_support::Json;

mod push_side_translation;
mod stalled_receive_timeouts;

/// Environment variables for a child process.
type Env = BTreeMap<OsString, OsString>;

/// A locale setting: the variables it sets, a later one winning over an earlier one of its name.
type Setting = Vec<(&'static str, &'static str)>;

/// The translated settings the review ran, plus the C control. Each blanks the
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

/// Whether git answers in another language than English under `env`: a
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

/// Shell lines giving a hook's own commit an identity, whatever the user's
/// git config holds.
const WINNER_IDENTITY: &str = concat!(
    "export GIT_AUTHOR_NAME=winner GIT_AUTHOR_EMAIL=w@example.invalid ",
    "GIT_COMMITTER_NAME=winner GIT_COMMITTER_EMAIL=w@example.invalid\n"
);

/// The member `key` of `value`; absence fails the test.
fn member<'v>(value: &'v Json, key: &str) -> &'v Json {
    value
        .get(key)
        .unwrap_or_else(|| panic!("no {key:?} in {value:?}"))
}
