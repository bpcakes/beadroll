//! The sync repository's git environment: remove caller repository-location variables, set bead's
//! identity, carry checkout transport settings and apply sync maintenance settings last.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
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
pub const DEFAULT_SSH_COMMAND: &str =
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
    let count = vars
        .get(count_var)
        .and_then(|value| value.to_str())
        .map_or_else(Decimal::zero, Decimal::read);
    for (index, (key, value)) in settings.iter().enumerate() {
        let at = count.plus(index);
        vars.insert(format!("GIT_CONFIG_KEY_{at}").into(), key.into());
        vars.insert(format!("GIT_CONFIG_VALUE_{at}").into(), value.clone());
    }
    vars.insert(count_var.into(), count.plus(settings.len()).into());
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
/// transmission. A valueless key means true.
pub fn transport_config(proc: &Output) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    if proc.success() {
        for item in String::from_utf8_lossy(&proc.stdout).split('\0') {
            if item.is_empty() {
                continue;
            }
            match item.split_once('\n') {
                Some((key, value)) => rows.push((key.to_owned(), value.to_owned())),
                None => rows.push((item.to_owned(), "true".to_owned())),
            }
        }
    }
    let mut resets: Vec<(String, String)> = Vec::new();
    for (key, _) in &rows {
        let resettable = RESETTABLE.as_ref().is_some_and(|re| re.is_match(key));
        if resettable && !resets.iter().any(|(seen, _)| seen == key) {
            resets.push((key.clone(), String::new()));
        }
    }
    resets.extend(rows);
    resets
}

/// Rules for the caller's GIT_CONFIG_COUNT: a digit predicate determines whether settings are
/// present, then exact integer parsing reads their count.
pub struct CountRules<'a> {
    /// Whether a string is nonempty and consists entirely of Unicode digits.
    pub is_digit: &'a dyn Fn(&str) -> bool,
    /// The exact parsed integer as decimal text, or `None` when unreadable.
    pub int: &'a dyn Fn(&str) -> Option<String>,
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
    carried: &[(String, String)],
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
    let count: Decimal = match env.get(count_var) {
        Some(value) => match value.to_str().filter(|text| (rules.is_digit)(text)) {
            Some(text) => Decimal::read(&(rules.int)(text).ok_or_else(|| refused(value))?),
            None => Decimal::zero(),
        },
        None => Decimal::zero(),
    };
    for (index, (key, value)) in carried.iter().enumerate() {
        let at = count.plus(index);
        env.insert(format!("GIT_CONFIG_KEY_{at}").into(), key.into());
        env.insert(format!("GIT_CONFIG_VALUE_{at}").into(), value.into());
    }
    if !carried.is_empty() {
        env.insert(count_var.into(), count.plus(carried.len()).into());
    }
    let has = |name: &str| env.contains_key(OsStr::new(name));
    if !has("GIT_SSH_COMMAND")
        && !has("GIT_SSH")
        && carried.iter().all(|(key, _)| key != "core.sshcommand")
    {
        env.insert("GIT_SSH_COMMAND".into(), DEFAULT_SSH_COMMAND.into());
    }
    // A missing or empty count means zero; other values must parse as integers.
    let count: Decimal = match env.get(count_var) {
        None => Decimal::zero(),
        Some(value) if value.is_empty() => Decimal::zero(),
        Some(value) => Decimal::read(
            &value
                .to_str()
                .and_then(|text| (rules.int)(text))
                .ok_or_else(|| refused(value))?,
        ),
    };
    for (index, (key, value)) in SYNC_REPO_CONFIG.iter().enumerate() {
        let at = count.plus(index);
        env.insert(format!("GIT_CONFIG_KEY_{at}").into(), (*key).into());
        env.insert(format!("GIT_CONFIG_VALUE_{at}").into(), (*value).into());
    }
    env.insert(count_var.into(), count.plus(SYNC_REPO_CONFIG.len()).into());
    Ok(checkout.with_vars(env))
}

/// An exact decimal count to which enumeration indices are added.
struct Decimal {
    negative: bool,
    /// Decimal digits, no leading zeros (`0` alone for zero).
    digits: String,
}

impl Decimal {
    fn zero() -> Decimal {
        Decimal {
            negative: false,
            digits: "0".into(),
        }
    }

    /// `text` as a canonical decimal integer: an optional minus, then digits.
    fn read(text: &str) -> Decimal {
        let (negative, digits) = match text.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        let digits = digits.trim_start_matches('0');
        if digits.is_empty() {
            return Decimal::zero();
        }
        Decimal {
            negative,
            digits: digits.into(),
        }
    }

    /// This number plus `n`, as decimal text.
    fn plus(&self, n: usize) -> String {
        let n = n.to_string();
        if !self.negative {
            return add_digits(&self.digits, &n);
        }
        let magnitude = (self.digits.len(), self.digits.as_str());
        match magnitude.cmp(&(n.len(), n.as_str())) {
            std::cmp::Ordering::Greater => format!("-{}", subtract_digits(&self.digits, &n)),
            _ => subtract_digits(&n, &self.digits),
        }
    }
}

/// The sum of two decimal magnitudes.
fn add_digits(a: &str, b: &str) -> String {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut out = Vec::new();
    let mut carry = 0u8;
    for at in 0..a.len().max(b.len()) {
        let digit = |x: &[u8]| {
            x.len()
                .checked_sub(at + 1)
                .and_then(|i| x.get(i))
                .map_or(0, |d| d - b'0')
        };
        let total = digit(a) + digit(b) + carry;
        out.push(b'0' + total % 10);
        carry = total / 10;
    }
    if carry > 0 {
        out.push(b'0' + carry);
    }
    out.reverse();
    trimmed(out)
}

/// The difference of two decimal magnitudes, the first not the smaller.
fn subtract_digits(a: &str, b: &str) -> String {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut out = Vec::new();
    let mut borrow = 0i8;
    for at in 0..a.len() {
        let digit = |x: &[u8]| {
            x.len()
                .checked_sub(at + 1)
                .and_then(|i| x.get(i))
                .map_or(0, |d| (d - b'0') as i8)
        };
        let mut value = digit(a) - digit(b) - borrow;
        borrow = 0;
        if value < 0 {
            value += 10;
            borrow = 1;
        }
        out.push(b'0' + value as u8);
    }
    out.reverse();
    trimmed(out)
}

/// Decimal digits without leading zeros (`0` for none).
fn trimmed(digits: Vec<u8>) -> String {
    let text = String::from_utf8_lossy(&digits);
    let text = text.trim_start_matches('0');
    if text.is_empty() {
        "0".into()
    } else {
        text.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ASCII numeric rules sufficient for these test cases.
    const RULES: CountRules<'static> = CountRules {
        is_digit: &|text| !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()),
        int: &|text| text.trim().parse::<i128>().ok().map(|n| n.to_string()),
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
            ("credential.helper".to_owned(), String::new()),
            ("credential.helper".to_owned(), "store".to_owned()),
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

    #[test]
    fn a_caller_ssh_setting_suppresses_the_default() {
        for vars in [&[("GIT_SSH", "")][..], &[("GIT_SSH_COMMAND", "x")][..]] {
            let env = sync_env(&checkout(vars), Path::new("/s"), &[], &RULES).unwrap();
            assert_ne!(get(&env, "GIT_SSH_COMMAND"), Some(DEFAULT_SSH_COMMAND));
        }
        let carried = [("core.sshcommand".to_owned(), "ssh -v".to_owned())];
        let env = sync_env(&checkout(&[]), Path::new("/s"), &carried, &RULES).unwrap();
        assert_eq!(get(&env, "GIT_SSH_COMMAND"), None);
    }

    #[test]
    fn counts_are_numbered_on_exactly() {
        assert_eq!(Decimal::read("-3").plus(4), "1");
        assert_eq!(Decimal::read("-3").plus(1), "-2");
        assert_eq!(Decimal::read("-3").plus(3), "0");
        assert_eq!(Decimal::read("007").plus(0), "7");
        let wide = "9".repeat(45);
        assert_eq!(Decimal::read(&wide).plus(1), format!("1{}", "0".repeat(45)));
        assert_eq!(
            Decimal::read(&format!("-1{}", "0".repeat(45))).plus(1),
            format!("-{}", "9".repeat(45))
        );
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
