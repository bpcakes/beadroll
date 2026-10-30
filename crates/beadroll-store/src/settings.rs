//! The settings bead runs with, built once by the CLI from the process (`CONTRIBUTING.md`):
//! no library reads the environment, the working directory or the clock offset itself.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use beadroll_core::text::{parse_float, parse_integer, repr_str};
use beadroll_git::{Checkout, Env, Fact, Output};

use crate::CrashHooks;
use crate::error::Error;
use crate::failpoint::{Failpoints, Fired};

/// Segment rotation size when `BEAD_ROTATE_BYTES` is unset.
pub const ROTATE_BYTES: i128 = 256 * 1024;
/// Seconds for one fetch or push when `BEAD_NET_TIMEOUT` is unset.
pub const NET_TIMEOUT: f64 = 45.0;
/// Loose objects in `sync.git` before bead repacks it, when `BEAD_REPACK_LOOSE` is unset.
pub const REPACK_LOOSE: i128 = 1000;
/// Packs in `sync.git` (one per fetch that brought something) before bead repacks it, when
/// `BEAD_REPACK_PACKS` is unset.
pub const REPACK_PACKS: i128 = 40;

/// The event-id and timestamp clock: system nanoseconds plus a configured offset, with the clock
/// failpoint applied at each reading (`offset(±DUR)` or `at(TIME)`).
#[derive(Clone, Debug, Default)]
pub struct Clock {
    offset_ns: i128,
    failpoints: Option<Arc<Failpoints>>,
}

impl Clock {
    /// The system clock as it is.
    pub fn system() -> Clock {
        Clock::default()
    }

    /// The system clock shifted by `offset_ns` nanoseconds.
    pub fn shifted(offset_ns: i128) -> Clock {
        Clock {
            offset_ns,
            failpoints: None,
        }
    }

    /// The system clock, as the `clock` failpoint of `failpoints` has it.
    pub fn with_failpoints(failpoints: Arc<Failpoints>) -> Clock {
        Clock {
            offset_ns: 0,
            failpoints: Some(failpoints),
        }
    }

    /// Nanoseconds since the epoch.
    pub fn now_ns(&self) -> i128 {
        let real = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(since) => since.as_nanos() as i128,
            Err(before) => -(before.duration().as_nanos() as i128),
        };
        let now = real + self.offset_ns;
        match self.failpoints.as_ref().and_then(|f| f.hit("clock")) {
            Some(Fired::Offset(ns)) => now + ns,
            Some(Fired::At(ns)) => ns,
            _ => now,
        }
    }
}

/// Everything bead takes from its process: the variables it reads, the environment git children
/// inherit, the working directory, and the clock.
#[derive(Clone, Debug)]
pub struct Settings {
    /// `BEAD_HOME` as set, and with a leading home-directory shorthand expanded.
    bead_home: Option<(OsString, OsString)>,
    user_home: OsString,
    /// `BEAD_ROTATE_BYTES`: the size from which this machine starts a new segment.
    pub rotate_bytes: i128,
    /// `BEAD_NET_TIMEOUT`: seconds for one fetch or push.
    pub net_timeout: f64,
    /// `BEAD_REPACK_LOOSE`: loose objects in `sync.git` before it is repacked.
    pub repack_loose: i128,
    /// `BEAD_REPACK_PACKS`: packs in `sync.git` before it is repacked.
    pub repack_packs: i128,
    /// The environment checkout-side git runs in, starting in `cwd`, and the one `sync.git`'s is
    /// derived from.
    pub git: Env,
    /// What checkout-side git says about the repository and its configuration, asked once in
    /// [`Settings::git`] (`crates/beadroll-git/AGENTS.md`): set `git` before the first question.
    pub checkout: Checkout,
    /// The working directory relative paths are read from.
    pub cwd: PathBuf,
    /// The clock event ids and stamps are read from.
    pub clock: Clock,
    /// The failpoints of this invocation (`BEAD_FAILPOINTS` and the crash hooks).
    pub failpoints: Arc<Failpoints>,
    /// Which build of bead runs, as the CLI knows it: a store's projection is read only by the
    /// build that wrote it, since another may replay otherwise (roll-lqjj.2). `None`: the store
    /// keeps no projection, and every command replays its journal.
    pub build: Option<String>,
}

impl Settings {
    /// Explicit process settings: inherited variables, working directory, home, temporary
    /// directory and a lookup for named users' homes. With no cwd, git inherits the process
    /// directory. HOME falls back to the password database; `home_of` supports ~NAME in
    /// BEAD_HOME. Invalid numeric settings return CONFIG.
    pub fn from_process(
        vars: BTreeMap<OsString, OsString>,
        cwd: Option<PathBuf>,
        user_home: OsString,
        tmp: PathBuf,
        home_of: &dyn Fn(&OsStr) -> Option<OsString>,
    ) -> Result<Settings, Error> {
        let get = |name: &str| vars.get(OsStr::new(name)).filter(|v| !v.is_empty());
        let int = |name: &str, default: i128| -> Result<i128, Error> {
            get(name).map_or(Ok(default), |value| {
                let read = value.to_str().and_then(parse_integer);
                read.map(|n| n.as_i128()).ok_or_else(|| {
                    Error::Config(format!(
                        "{name} must be a whole number, not {}",
                        repr_str(&value.to_string_lossy())
                    ))
                })
            })
        };
        let net_timeout = get("BEAD_NET_TIMEOUT").map_or(Ok(NET_TIMEOUT), |value| {
            value.to_str().and_then(parse_float).ok_or_else(|| {
                Error::Config(format!(
                    "BEAD_NET_TIMEOUT must be a number of seconds, not {}",
                    repr_str(&value.to_string_lossy())
                ))
            })
        })?;
        let bead_home =
            get("BEAD_HOME").map(|value| (value.clone(), expand_user(value, &user_home, home_of)));
        let failpoints = Arc::new(Failpoints::from_env(
            get("BEAD_FAILPOINTS").map(OsString::as_os_str),
            CrashHooks {
                before_push: get("BEAD_TEST_CRASH_BEFORE_PUSH").is_some(),
                after_push: get("BEAD_TEST_CRASH_AFTER_PUSH").is_some(),
            },
        )?);
        Ok(Settings {
            bead_home,
            rotate_bytes: int("BEAD_ROTATE_BYTES", ROTATE_BYTES)?,
            net_timeout,
            repack_loose: int("BEAD_REPACK_LOOSE", REPACK_LOOSE)?,
            repack_packs: int("BEAD_REPACK_PACKS", REPACK_PACKS)?,
            // git starts where relative paths are read from: the checkout the caller names
            git: Env::new(vars.clone(), cwd.clone(), tmp),
            checkout: Checkout::default(),
            cwd: cwd.unwrap_or_else(|| PathBuf::from(".")),
            user_home,
            clock: Clock::with_failpoints(Arc::clone(&failpoints)),
            failpoints,
            build: None,
        })
    }

    /// A supplied environment variable, if set and nonempty.
    pub fn var(&self, name: &str) -> Option<&OsStr> {
        self.git.var(name).filter(|value| !value.is_empty())
    }

    /// What the checkout's `git rev-parse` prints for `fact` alone ([`Checkout::fact`]).
    pub fn fact(&self, fact: Fact) -> Result<Output, beadroll_git::Error> {
        self.checkout.fact(&self.git, fact)
    }

    /// The store's root: `BEAD_HOME` (`~` expanded), else `~/.local/state/beadroll`. A `BEAD_HOME`
    /// that is not absolute is refused: the store never lives in a checkout.
    pub fn home(&self) -> Result<PathBuf, Error> {
        let Some((value, expanded)) = &self.bead_home else {
            return Ok(
                normalize_path(&expand_user(OsStr::new("~"), &self.user_home, &|_| None))
                    .join(".local")
                    .join("state")
                    .join("beadroll"),
            );
        };
        let path = normalize_path(expanded);
        if !path.is_absolute() {
            return Err(Error::Config(format!(
                "BEAD_HOME must be an absolute path, not {}: the store never lives in a checkout",
                repr_str(&value.to_string_lossy())
            )));
        }
        Ok(path)
    }
}

/// Expands a leading `~` to the user's home directory, or `~NAME` to that of the user `NAME`
/// as `home_of` finds it (unchanged when there is no such user); the home directory without its
/// trailing slashes, then the rest; `/` when that leaves nothing.
fn expand_user(
    path: &OsStr,
    user_home: &OsStr,
    home_of: &dyn Fn(&OsStr) -> Option<OsString>,
) -> OsString {
    let bytes = path.as_bytes();
    if !bytes.starts_with(b"~") {
        return path.to_owned();
    }
    let end = bytes[1..]
        .iter()
        .position(|b| *b == b'/')
        .map_or(bytes.len(), |at| at + 1);
    let named;
    let home = if end == 1 {
        user_home
    } else {
        match home_of(OsStr::from_bytes(&bytes[1..end])) {
            Some(found) => {
                named = found;
                named.as_os_str()
            }
            None => return path.to_owned(),
        }
    };
    let mut home = home.as_bytes();
    while let Some(trimmed) = home.strip_suffix(b"/") {
        home = trimmed;
    }
    let expanded = [home, &bytes[end..]].concat();
    if expanded.is_empty() {
        return OsString::from("/");
    }
    OsString::from_vec(expanded)
}

/// Normalises a path by removing empty and dot components and trailing slashes. Exactly two
/// leading slashes remain; three or more become one.
pub fn normalize_path(path: &OsStr) -> PathBuf {
    let bytes = path.as_bytes();
    let root: &[u8] = if bytes.starts_with(b"//") && !bytes.starts_with(b"///") {
        b"//"
    } else if bytes.starts_with(b"/") {
        b"/"
    } else {
        b""
    };
    let parts: Vec<&[u8]> = bytes
        .split(|b| *b == b'/')
        .filter(|part| !part.is_empty() && *part != b".")
        .collect();
    let joined = parts.join(&b'/');
    let out = [root, &joined].concat();
    if out.is_empty() {
        return PathBuf::from(".");
    }
    PathBuf::from(OsString::from_vec(out))
}

/// Joins base and rest, normalising the result. An absolute rest replaces base.
pub fn join_path(base: &Path, rest: &OsStr) -> PathBuf {
    if rest.as_bytes().starts_with(b"/") {
        return normalize_path(rest);
    }
    let joined = [base.as_os_str().as_bytes(), b"/", rest.as_bytes()].concat();
    normalize_path(OsStr::from_bytes(&joined))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(vars: &[(&str, &str)]) -> Result<Settings, Error> {
        Settings::from_process(
            vars.iter()
                .map(|(k, v)| (OsString::from(k), OsString::from(v)))
                .collect(),
            Some(PathBuf::from("/w")),
            OsString::from("/srv/u/"),
            PathBuf::from("/tmp"),
            &|name| (name == "alice").then(|| OsString::from("/srv/alice//")),
        )
    }

    #[test]
    fn the_store_root_follows_bead_home_rules() {
        let home = |vars: &[(&str, &str)]| settings(vars).unwrap().home();
        assert_eq!(
            home(&[]).unwrap(),
            Path::new("/srv/u/.local/state/beadroll")
        );
        assert_eq!(
            home(&[("BEAD_HOME", "")]).unwrap(),
            Path::new("/srv/u/.local/state/beadroll")
        );
        assert_eq!(
            home(&[("BEAD_HOME", "~/s//x/./")]).unwrap(),
            Path::new("/srv/u/s/x")
        );
        let refused = home(&[("BEAD_HOME", "rel")]).unwrap_err();
        assert_eq!(
            refused.to_string(),
            "BEAD_HOME must be an absolute path, not 'rel': the store never lives in a checkout"
        );
        assert_eq!(
            home(&[("BEAD_HOME", "~x/y")]).unwrap_err().code().as_str(),
            "CONFIG"
        );
    }

    #[test]
    fn numeric_settings_accept_supported_syntax_and_refuse_the_rest() {
        let s = settings(&[
            ("BEAD_ROTATE_BYTES", " 10 "),
            ("BEAD_NET_TIMEOUT", "0.25"),
            ("BEAD_REPACK_LOOSE", ""),
        ])
        .unwrap();
        assert_eq!(
            (s.rotate_bytes, s.net_timeout, s.repack_loose),
            (10, 0.25, 1000)
        );
        let bad = settings(&[("BEAD_NET_TIMEOUT", "soon")]).unwrap_err();
        assert_eq!(bad.code().as_str(), "CONFIG");
        assert!(settings(&[("BEAD_REPACK_PACKS", "1.5")]).is_err());
        // beyond what bead counts (an i128), a setting is refused as any other unreadable one
        let wide = "9".repeat(60);
        let refused = settings(&[("BEAD_ROTATE_BYTES", wide.as_str())]).unwrap_err();
        assert_eq!(refused.code().as_str(), "CONFIG");
        let most = i128::MAX.to_string();
        let read = settings(&[("BEAD_ROTATE_BYTES", most.as_str())]).unwrap();
        assert_eq!(read.rotate_bytes, i128::MAX);
        assert!(settings(&[("BEAD_ROTATE_BYTES", " 1_0 ")]).is_err());
        assert_eq!(
            settings(&[("BEAD_ROTATE_BYTES", " +10 ")])
                .unwrap()
                .rotate_bytes,
            10
        );
    }

    #[test]
    fn paths_normalize_separators_and_current_directory_components() {
        assert_eq!(normalize_path(OsStr::new("//a/./b//")), Path::new("//a/b"));
        assert_eq!(normalize_path(OsStr::new("///a")), Path::new("/a"));
        assert_eq!(
            join_path(Path::new("/r"), OsStr::new("./x/")),
            Path::new("/r/x")
        );
        assert_eq!(
            join_path(Path::new("/r"), OsStr::new("/abs")),
            Path::new("/abs")
        );
        assert_eq!(expand_user(OsStr::new("~"), OsStr::new(""), &|_| None), "/");
    }
}
