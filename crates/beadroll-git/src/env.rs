//! The sync repository's git environment: remove caller repository-location variables, set bead's
//! identity, carry checkout transport settings and apply sync maintenance settings last.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt as _;
use std::path::Path;
use std::sync::LazyLock;

use regex_lite::Regex;

use crate::error::Error;
use crate::runner::{Env, Output};

/// The keys that say how git reaches remotes from the checkout (`git config --get-regexp`).
pub const TRANSPORT_KEYS: &str =
    r"^(core\.sshcommand|ssh\.variant|http\..+|credential\..+|url\..+\.(insteadof|pushinsteadof))$";

/// Where the caller's repository is (`git rev-parse --local-env-vars`, less the `-c` settings, which
/// stay the caller's): none of it may reach `sync.git`.
pub const REPO_LOCATION_ENV: [&str; 14] = [
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_OBJECT_DIRECTORY",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_GRAFT_FILE",
    "GIT_INDEX_FILE",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_REPLACE_REF_BASE",
    "GIT_PREFIX",
    "GIT_SHALLOW_FILE",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
    "GIT_QUARANTINE_PATH",
];

/// `sync.git` is bead's own: no detached git maintenance racing the next command (bead repacks it
/// itself, under the store lock), and what a fetch brings stays in the pack index-pack verified and
/// wrote whole, never exploded into loose files.
pub const SYNC_REPO_CONFIG: [(&str, &str); 4] = [
    ("gc.auto", "0"),
    ("gc.autoDetach", "false"),
    ("maintenance.auto", "false"),
    ("fetch.unpackLimit", "1"),
];

/// The ssh command bead sets when the caller has none: a dead remote must fail, not hang.
pub(crate) const DEFAULT_SSH_COMMAND: &str =
    "ssh -o ConnectTimeout=15 -o ServerAliveInterval=15 -o ServerAliveCountMax=2";

/// Lists that an empty value clears (`credential.helper`, `http.extraHeader`, also per URL).
static RESETTABLE: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"^(credential\.(.+\.)?helper|http\.(.+\.)?extraheader)$").ok());

/// `env`, a link's ([`sync_env`]), with `settings` given after its `GIT_CONFIG_COUNT` settings: at
/// command-line precedence, as `git -c KEY=VALUE` gives them, but in the environment, so that
/// git's command stays its first argument, where the tests' wrappers and accounts look for it.
pub fn with_settings(env: &Env, settings: &[(String, OsString)]) -> Env {
    let mut vars = env.vars().clone();
    let count_var = OsStr::new("GIT_CONFIG_COUNT");
    // the count `sync_env` wrote, an integer it has checked has room for its own settings
    let count = vars
        .get(count_var)
        .and_then(|value| value.to_str())
        .and_then(|text| text.trim().parse::<i128>().ok())
        .unwrap_or(0);
    let at = |n: usize| count.saturating_add(i128::try_from(n).unwrap_or(i128::MAX));
    for (index, (key, value)) in settings.iter().enumerate() {
        let at = at(index);
        vars.insert(format!("GIT_CONFIG_KEY_{at}").into(), key.into());
        vars.insert(format!("GIT_CONFIG_VALUE_{at}").into(), value.clone());
    }
    vars.insert(count_var.into(), at(settings.len()).to_string().into());
    env.with_vars(vars)
}

/// Reads TRANSPORT_KEYS with git config when each link is created, so changes made while waiting
/// for the store lock are observed.
pub fn transport_settings(env: &Env) -> Result<Output, Error> {
    env.git(["config", "--null", "--get-regexp", TRANSPORT_KEYS])
        .run()
}

/// Checkout transport configuration, including repository, conditional-include and global
/// settings. The private sync repository cannot see all of these itself. Apply them at
/// command-line precedence in order, clearing resettable lists first to avoid duplicate
/// transmission. A valueless key means true. Keys and values are carried as git gave them, bytes and
/// all: a path or URL that is not UTF-8 reaches the sync repository as it is, never guessed at.
pub fn transport_config(proc: &Output) -> Vec<(OsString, OsString)> {
    let mut rows = Vec::new();
    if proc.success() {
        for item in proc.stdout.split(|byte| *byte == 0) {
            if item.is_empty() {
                continue;
            }
            let bytes = |bytes: &[u8]| OsStr::from_bytes(bytes).to_owned();
            match item.iter().position(|byte| *byte == b'\n') {
                Some(at) => rows.push((bytes(&item[..at]), bytes(&item[at + 1..]))),
                None => rows.push((bytes(item), OsString::from("true"))),
            }
        }
    }
    let mut resets: Vec<(OsString, OsString)> = Vec::new();
    for (key, _) in &rows {
        let resettable = (RESETTABLE.as_ref())
            .is_some_and(|re| key.to_str().is_some_and(|key| re.is_match(key)));
        if resettable && !resets.iter().any(|(seen, _)| seen == key) {
            resets.push((key.clone(), OsString::new()));
        }
    }
    resets.extend(rows);
    resets
}

/// Rules for the caller's GIT_CONFIG_COUNT: a digit predicate determines whether settings are
/// present, then exact integer parsing reads their count.
pub struct CountRules<'a> {
    /// Whether a string is nonempty and consists entirely of the caller's accepted digits.
    pub is_digit: &'a dyn Fn(&str) -> bool,
    /// The integer, or `None` when it is unreadable or does not fit an `i128`.
    pub int: &'a dyn Fn(&str) -> Option<i128>,
}

/// Builds the sync environment from the checkout's: remove repository-location variables; set
/// GIT_DIR, disabled prompting, C locale and bead identity; append carried transport settings
/// after the caller's GIT_CONFIG_COUNT entries; set default SSH only when absent; apply
/// SYNC_REPO_CONFIG last.
///
/// An unreadable count returns [`Error::ConfigCount`] rather than silently dropping caller
/// settings.
pub fn sync_env(
    checkout: &Env,
    git_dir: &Path,
    carried: &[(OsString, OsString)],
    rules: &CountRules<'_>,
) -> Result<Env, Error> {
    let mut env: BTreeMap<OsString, OsString> = checkout
        .vars()
        .iter()
        .filter(|(name, _)| {
            !REPO_LOCATION_ENV
                .iter()
                .any(|gone| OsStr::new(gone) == *name)
        })
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    let mut set = |name: &str, value: &OsStr| {
        env.insert(name.into(), value.to_owned());
    };
    set("GIT_DIR", git_dir.as_os_str());
    for (name, value) in [
        ("GIT_TERMINAL_PROMPT", "0"),
        ("LC_ALL", "C"),
        ("GIT_AUTHOR_NAME", "bead"),
        ("GIT_AUTHOR_EMAIL", "bead@localhost"),
        ("GIT_COMMITTER_NAME", "bead"),
        ("GIT_COMMITTER_EMAIL", "bead@localhost"),
    ] {
        set(name, OsStr::new(value));
    }
    let count_var = OsStr::new("GIT_CONFIG_COUNT");
    let refused = |value: &OsStr| Error::ConfigCount(value.to_string_lossy().into_owned());
    // Only an all-digit count is parsed; missing or non-digit counts mean zero.
    let given = env.get(count_var).cloned();
    let count: i128 = match &given {
        Some(value) => match value.to_str().filter(|text| (rules.is_digit)(text)) {
            Some(text) => (rules.int)(text).ok_or_else(|| refused(value))?,
            None => 0,
        },
        None => 0,
    };
    // the index `n` places after the caller's settings; a count without room for them is refused
    let after = |count: i128, n: usize| {
        i128::try_from(n)
            .ok()
            .and_then(|n| count.checked_add(n))
            .ok_or_else(|| refused(given.as_deref().unwrap_or_default()))
    };
    for (index, (key, value)) in carried.iter().enumerate() {
        let at = after(count, index)?;
        env.insert(format!("GIT_CONFIG_KEY_{at}").into(), key.into());
        env.insert(format!("GIT_CONFIG_VALUE_{at}").into(), value.into());
    }
    if !carried.is_empty() {
        let total = after(count, carried.len())?;
        env.insert(count_var.into(), total.to_string().into());
    }
    let has = |name: &str| env.contains_key(OsStr::new(name));
    if !has("GIT_SSH_COMMAND")
        && !has("GIT_SSH")
        && carried.iter().all(|(key, _)| key != "core.sshcommand")
    {
        env.insert("GIT_SSH_COMMAND".into(), DEFAULT_SSH_COMMAND.into());
    }
    // A missing or empty count means zero; other values must parse as integers.
    let count: i128 = match env.get(count_var) {
        None => 0,
        Some(value) if value.is_empty() => 0,
        Some(value) => value
            .to_str()
            .and_then(|text| (rules.int)(text))
            .ok_or_else(|| refused(value))?,
    };
    for (index, (key, value)) in SYNC_REPO_CONFIG.iter().enumerate() {
        let at = after(count, index)?;
        env.insert(format!("GIT_CONFIG_KEY_{at}").into(), (*key).into());
        env.insert(format!("GIT_CONFIG_VALUE_{at}").into(), (*value).into());
    }
    let total = after(count, SYNC_REPO_CONFIG.len())?;
    env.insert(count_var.into(), total.to_string().into());
    Ok(checkout.with_vars(env))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A transport setting that is not UTF-8 (a cookie file's path, a URL's subsection) is carried
    /// as git gave it, bytes and all, never with U+FFFD in place of what it could not decode; a
    /// valueless key is true.
    #[test]
    fn transport_settings_are_carried_as_bytes() {
        let answered = Output {
            ended: crate::runner::Ended::Exit(0),
            stdout: b"http.cookiefile\n/tmp/caf\xe9\0url.\xff.insteadof\nx\0http.sslverify\0"
                .to_vec(),
            stderr: Vec::new(),
        };
        let rows = transport_config(&answered);
        let bytes = |text: &OsString| text.as_bytes().to_vec();
        let rows: Vec<(Vec<u8>, Vec<u8>)> =
            (rows.iter()).map(|(k, v)| (bytes(k), bytes(v))).collect();
        assert!(
            rows.contains(&(b"http.cookiefile".to_vec(), b"/tmp/caf\xe9".to_vec())),
            "{rows:?}"
        );
        assert!(
            rows.contains(&(b"url.\xff.insteadof".to_vec(), b"x".to_vec())),
            "{rows:?}"
        );
        assert!(
            rows.contains(&(b"http.sslverify".to_vec(), b"true".to_vec())),
            "{rows:?}"
        );
    }

    /// ASCII numeric rules sufficient for these test cases.
    const RULES: CountRules<'static> = CountRules {
        is_digit: &|text| !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()),
        int: &|text| text.trim().parse::<i128>().ok(),
    };

    fn checkout(vars: &[(&str, &str)]) -> Env {
        Env::new(
            vars.iter()
                .map(|(k, v)| (OsString::from(k), OsString::from(v))),
            None,
            std::env::temp_dir(),
        )
    }

    fn get<'a>(env: &'a Env, name: &str) -> Option<&'a str> {
        env.var(name).and_then(OsStr::to_str)
    }

    #[test]
    fn the_sync_environment_is_built_as_link_builds_it() {
        let base = checkout(&[
            ("GIT_DIR", "/elsewhere"),
            ("GIT_WORK_TREE", "/w"),
            ("GIT_CONFIG_COUNT", "1"),
            ("GIT_CONFIG_KEY_0", "user.name"),
            ("GIT_CONFIG_VALUE_0", "x"),
            ("HOME", "/h"),
        ]);
        let carried = vec![
            (OsString::from("credential.helper"), OsString::new()),
            (OsString::from("credential.helper"), OsString::from("store")),
        ];
        let env = sync_env(&base, Path::new("/s/sync.git"), &carried, &RULES).unwrap();
        assert_eq!(get(&env, "GIT_DIR"), Some("/s/sync.git"));
        assert_eq!(get(&env, "GIT_WORK_TREE"), None);
        assert_eq!(get(&env, "HOME"), Some("/h"));
        assert_eq!(get(&env, "LC_ALL"), Some("C"));
        assert_eq!(get(&env, "GIT_CONFIG_KEY_1"), Some("credential.helper"));
        assert_eq!(get(&env, "GIT_CONFIG_VALUE_2"), Some("store"));
        assert_eq!(get(&env, "GIT_CONFIG_KEY_3"), Some("gc.auto"));
        assert_eq!(get(&env, "GIT_CONFIG_KEY_6"), Some("fetch.unpackLimit"));
        assert_eq!(get(&env, "GIT_CONFIG_COUNT"), Some("7"));
        assert_eq!(get(&env, "GIT_SSH_COMMAND"), Some(DEFAULT_SSH_COMMAND));
    }

    /// The pattern is held as an `Option`, since a library may not panic; a pattern that did not
    /// compile would answer "not resettable" of every key, so this is where it fails.
    #[test]
    fn the_resettable_lists_pattern_compiles_and_matches_them() {
        let re = RESETTABLE.as_ref().expect("RESETTABLE compiles");
        for key in [
            "credential.helper",
            "credential.https://h.helper",
            "http.extraheader",
        ] {
            assert!(re.is_match(key), "{key}");
        }
        assert!(!re.is_match("credential.username"));
    }

    #[test]
    fn a_caller_ssh_setting_suppresses_the_default() {
        for vars in [&[("GIT_SSH", "")][..], &[("GIT_SSH_COMMAND", "x")][..]] {
            let env = sync_env(&checkout(vars), Path::new("/s"), &[], &RULES).unwrap();
            assert_ne!(get(&env, "GIT_SSH_COMMAND"), Some(DEFAULT_SSH_COMMAND));
        }
        let carried = [(OsString::from("core.sshcommand"), OsString::from("ssh -v"))];
        let env = sync_env(&checkout(&[]), Path::new("/s"), &carried, &RULES).unwrap();
        assert_eq!(get(&env, "GIT_SSH_COMMAND"), None);
    }

    #[test]
    fn counts_are_numbered_on_exactly() {
        for (given, first, total) in [("007", "7", "11"), ("-3", "-3", "1"), ("-2", "-2", "2")] {
            let env = sync_env(
                &checkout(&[("GIT_CONFIG_COUNT", given)]),
                Path::new("/s"),
                &[],
                &RULES,
            )
            .unwrap();
            assert_eq!(
                get(&env, &format!("GIT_CONFIG_KEY_{first}")),
                Some("gc.auto"),
                "{given}"
            );
            assert_eq!(get(&env, "GIT_CONFIG_COUNT"), Some(total), "{given}");
        }
        let near = (i128::MAX - 4).to_string();
        let env = sync_env(
            &checkout(&[("GIT_CONFIG_COUNT", &near)]),
            Path::new("/s"),
            &[],
            &RULES,
        );
        assert_eq!(
            get(&env.unwrap(), "GIT_CONFIG_COUNT"),
            Some(i128::MAX.to_string().as_str())
        );
    }

    /// A count with no room for bead's own settings after it is refused, not wrapped or written
    /// past what git can read.
    #[test]
    fn a_count_without_room_for_beads_settings_is_refused() {
        let full = (i128::MAX - 3).to_string();
        let error = sync_env(
            &checkout(&[("GIT_CONFIG_COUNT", &full)]),
            Path::new("/s"),
            &[],
            &RULES,
        );
        assert!(matches!(error, Err(Error::ConfigCount(_))), "{error:?}");
    }

    #[test]
    fn settings_are_added_after_the_count_the_link_has() {
        let env = checkout(&[("GIT_CONFIG_COUNT", "5")]);
        let added = with_settings(&env, &[("remote.r.url".to_owned(), OsString::from("u"))]);
        assert_eq!(get(&added, "GIT_CONFIG_KEY_5"), Some("remote.r.url"));
        assert_eq!(get(&added, "GIT_CONFIG_COUNT"), Some("6"));
    }

    #[test]
    fn counts_reject_invalid_numbers_and_accept_surrounding_whitespace() {
        let env = sync_env(
            &checkout(&[("GIT_CONFIG_COUNT", " 2 ")]),
            Path::new("/s"),
            &[],
            &RULES,
        )
        .unwrap();
        assert_eq!(get(&env, "GIT_CONFIG_KEY_2"), Some("gc.auto"));
        assert_eq!(get(&env, "GIT_CONFIG_COUNT"), Some("6"));
        let error = sync_env(
            &checkout(&[("GIT_CONFIG_COUNT", "x")]),
            Path::new("/s"),
            &[],
            &RULES,
        );
        assert!(matches!(error, Err(Error::ConfigCount(_))));
    }
}
