//! The environment `sync.git`'s git runs in (Python's `Link.__init__`): the caller's environment
//! without anything that locates the caller's repository, bead's own identity, the checkout's
//! transport settings carried over, and `sync.git`'s own maintenance settings last.

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

/// What `git config --null --get-regexp` prints for [`TRANSPORT_KEYS`] in the checkout that `env`
/// runs git in, asked now: Python's `transport_config` asks it each time a link is made, so a
/// setting changed while bead waited for the store lock is the one the link uses.
pub fn transport_settings(env: &Env) -> Result<Output, Error> {
    env.git(["config", "--null", "--get-regexp", TRANSPORT_KEYS])
        .run()
}

/// How git reaches remotes from the checkout (Python's `transport_config`), from what `git config
/// --null --get-regexp` printed for [`TRANSPORT_KEYS`] there ([`transport_settings`]): the caller's
/// repository configuration, its includeIf includes and the global ones, which `sync.git`
/// (elsewhere, with no remotes) does not all see. Given at command-line precedence in this order,
/// these make `sync.git`'s git reach the remote as the caller's does: lists that can be reset are
/// emptied first, so nothing is sent twice. A key without a value means `true`.
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

/// How `GIT_CONFIG_COUNT` is read, as Python reads it: `str.isdigit()` decides whether the caller
/// has settings of its own, and `int()` reads their number.
pub struct CountRules<'a> {
    /// Python's `value.isdigit()`.
    pub is_digit: &'a dyn Fn(&str) -> bool,
    /// Python's `str(int(value))`, exact, or `None` where `int()` raises.
    pub int: &'a dyn Fn(&str) -> Option<String>,
}

/// The environment of `sync.git` at `git_dir`, from the checkout's: the repository-location
/// variables gone; `GIT_DIR`, no terminal prompt, the C locale and bead's identity set; `carried`
/// (the checkout's transport settings) after any `GIT_CONFIG_COUNT` settings the caller has; the
/// default ssh command only when the caller names none; [`SYNC_REPO_CONFIG`] last, so it wins.
///
/// A `GIT_CONFIG_COUNT` that `int()` cannot read, where Python reads it, fails here with
/// [`Error::ConfigCount`], where Python dies with a traceback (`docs/RUST-PORT.md` §5 row 26).
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
    // `int(count) if count.isdigit() else 0`
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
    // `int(env.get("GIT_CONFIG_COUNT") or 0)`.
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

/// A whole number as Python's `str(int)` writes it, to which `enumerate` adds the index exactly.
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

    /// `text` as `str(int(...))` gives it: an optional minus, then digits.
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

    /// Python's rules, for ASCII text: enough for these cases.
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
    fn a_count_that_is_not_a_number_is_an_error_and_spaces_are_read_as_python_reads_them() {
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
