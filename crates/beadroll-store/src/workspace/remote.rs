//! Remote resolution: what a remote given to bead (a configured remote's name, a URL, a path)
//! means, the one coordination endpoint a store goes through, and the remote's URL as git is
//! configured with it, read as discovery reads origin's.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};

use beadroll_core::json::Value;
use beadroll_git::{Fact, exists, is_local_path, normalize_remote, realpath, without_password};

use super::Workspace;
use crate::error::Error;

/// Where a store coordinates ([`Workspace::endpoint`]): the URL bead fetches from and publishes to,
/// and whether it is the push URL of its own (`remote.NAME.pushurl`) of the remote it was chosen
/// by, which git pushes to as configured, without the `pushInsteadOf` rewrites it applies to any
/// other URL, and so does bead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    /// The URL, as bead hands it to git.
    pub url: OsString,
    /// Whether it is the remote's own push URL.
    pub own_push_url: bool,
}

impl Endpoint {
    /// The endpoint at `url`, given by its URL alone: git applies its rewrites to it as to any URL.
    pub fn at(url: impl Into<OsString>) -> Endpoint {
        Endpoint {
            url: url.into(),
            own_push_url: false,
        }
    }
}

impl Workspace {
    /// Whether `url` is this checkout's origin, however it is spelled.
    pub fn is_origin(&self, url: &OsStr) -> bool {
        let Some(origin) = &self.origin else {
            return false;
        };
        if url.is_empty() || origin.is_empty() {
            return false;
        }
        let cwd = &self.settings.cwd;
        normalize_remote(url, cwd) == normalize_remote(origin, cwd)
    }

    /// The remote last named here. Origin itself is never remembered (older versions saved it
    /// too): it is looked up each time, so `git remote set-url origin` is followed.
    pub fn remembered_remote(&self) -> Result<Option<OsString>, Error> {
        let meta = self.meta(true)?;
        Ok(match meta.get("remote") {
            Some(Value::Str(remote)) if !self.is_origin(OsStr::new(remote)) => {
                Some(OsString::from(remote))
            }
            _ => None,
        })
    }

    /// One remote spelling (a configured remote's name, a URL, a path) reduced to the URL bead
    /// uses ([`Workspace::resolve_endpoint`]).
    pub fn resolve_remote(&self, value: &OsStr, typed: bool) -> Result<OsString, Error> {
        Ok(self.resolve_endpoint(value, typed)?.url)
    }

    /// One remote spelling (a configured remote's name, a URL, a path) reduced to the one bead
    /// uses, as git would read it: a configured remote wins over a same-named path, and is its own
    /// push URL when it has one; a relative path is read from the top of the checkout when it is
    /// stored, from the cwd when it was typed just now (`typed`), whether it exists or not (git
    /// reaches remotes from the top of the checkout), unless git rewrites it (`insteadOf`). A
    /// password is dropped.
    pub fn resolve_endpoint(&self, value: &OsStr, typed: bool) -> Result<Endpoint, Error> {
        if name_shaped(value)
            && let Some(Configured { url, push }) = configured_remote(&self.settings.git, value)?
        {
            // where `git push NAME` goes, as configured, as discovery reads origin (G22): git
            // applies `insteadOf` itself when bead hands it that URL, and `pushInsteadOf` unless
            // it is the remote's own push URL, as for `git push NAME`
            let mut endpoint = self.endpoint_at(&url, false)?;
            endpoint.own_push_url = push;
            return Ok(endpoint);
        }
        self.endpoint_at(value, typed)
    }

    /// Whether `value` names a remote configured here, which wins over a same-named path.
    pub fn names_remote(&self, value: &OsStr) -> Result<bool, Error> {
        Ok(name_shaped(value) && configured_remote(&self.settings.git, value)?.is_some())
    }

    /// Whether git rewrites `url` before it reaches it: an `insteadOf` or `pushInsteadOf` rule
    /// configured here is a prefix of it.
    pub fn rewrites(&self, url: &OsStr) -> Result<bool, Error> {
        rewritten(&self.settings.git, url)
    }

    /// `value`, a URL or a path and never a remote's name (a URL bead resolved already, remembered
    /// or read from git, may be spelled like one), as [`Workspace::resolve_endpoint`] reads one.
    fn endpoint_at(&self, value: &OsStr, typed: bool) -> Result<Endpoint, Error> {
        let git = &self.settings.git;
        let own_push_url = false;
        let value = without_password(value).0;
        let mut value = value;
        if is_local_path(&value) && !value.as_bytes().starts_with(b"/") {
            if rewritten(git, &value)? {
                // a path in spelling only: git makes a URL of it, which it could no longer do to
                // the path bead would make of it
                return Ok(Endpoint {
                    url: value,
                    own_push_url,
                });
            }
            let base = if typed {
                Some(self.settings.cwd.as_os_str().as_bytes().to_vec())
            } else {
                let top = self.settings.fact(Fact::Toplevel)?;
                top.success()
                    .then(|| crate::strip_ascii(&top.stdout).to_vec())
            };
            if let Some(base) = base {
                value = joined(base, &value);
            }
        }
        let cwd = &self.settings.cwd;
        let url = if exists(&value, cwd) {
            realpath(&value, cwd).into_os_string()
        } else {
            value
        };
        Ok(Endpoint { url, own_push_url })
    }

    /// The URL of the one coordination endpoint ([`Workspace::endpoint`]).
    pub fn remote(&self, override_: Option<&OsStr>) -> Result<Option<OsString>, Error> {
        Ok(self.endpoint(override_)?.map(|endpoint| endpoint.url))
    }

    /// The one coordination endpoint: `override` (`--remote`), else `BEAD_REMOTE`, the tracked
    /// configuration, the remote last named here, then origin, each with how it was chosen (a
    /// remote's own push URL or not; the remote last named here as it was saved beside it). A
    /// tracked configuration seeds a new store's remote, but never silently moves an existing
    /// store's.
    pub fn endpoint(&self, override_: Option<&OsStr>) -> Result<Option<Endpoint>, Error> {
        let typed = override_
            .filter(|value| !value.is_empty())
            .or_else(|| self.settings.var("BEAD_REMOTE"));
        if let Some(typed) = typed {
            return self.resolve_endpoint(typed, true).map(Some);
        }
        let tracked = self
            .config_str("remote")
            .filter(|tracked| !tracked.is_empty())
            .map(OsString::from);
        let used = match self.remembered_remote() {
            Ok(used) => used.filter(|used| !used.is_empty()),
            Err(error) if tracked.is_none() => return Err(error),
            Err(_) => None, // a damaged meta.json: only fatal when it is the sole record
        };
        if let (Some(tracked), Some(used)) = (&tracked, &used)
            && !self.leads_to(tracked, used)?
        {
            return Err(Error::Config(format!(
                ".beadroll.json names coordination remote {}, but this tracker already coordinates \
                 through {}; change it deliberately with `bead sync --remote` or `bead init \
                 --tracked --remote`",
                without_password(tracked).0.to_string_lossy(),
                used.to_string_lossy()
            )));
        }
        if let Some(tracked) = tracked {
            return self.resolve_endpoint(&tracked, false).map(Some);
        }
        if let Some(used) = used {
            let mut endpoint = self.endpoint_at(&used, false)?;
            endpoint.own_push_url = self.remembered_push(&used);
            return Ok(Some(endpoint));
        }
        match self.origin.clone().filter(|origin| !origin.is_empty()) {
            None => Ok(None),
            Some(origin) => {
                let mut endpoint = self.endpoint_at(&origin, false)?;
                endpoint.own_push_url = self.origin_push;
                Ok(Some(endpoint))
            }
        }
    }

    /// Whether `used`, the remote last named here, was a remote's own push URL when it was chosen:
    /// `remote_push_url` in `meta.json`, saved beside it ([`Workspace::note_sync`]), names it.
    fn remembered_push(&self, used: &OsStr) -> bool {
        let Ok(meta) = self.meta(false) else {
            return false;
        };
        matches!(meta.get("remote_push_url"), Some(Value::Str(pushed)) if OsStr::new(pushed) == used)
    }

    /// Whether the tracked remote `tracked` leads to `used`, the one this store last went through:
    /// resolved as now, or, for a remote's name, as older versions resolved and saved it, its URL
    /// as git rewrites it for fetching (`git remote get-url`, `insteadOf` applied). That is the
    /// same remote spelled another way, unless the name has a push URL of its own: they went
    /// through its other URL then, and moving to the push URL is done deliberately.
    fn leads_to(&self, tracked: &OsStr, used: &OsStr) -> Result<bool, Error> {
        if self.resolve_remote(tracked, false)? == used {
            return Ok(true);
        }
        let git = &self.settings.git;
        let own_push_url = if name_shaped(tracked) {
            configured_remote(git, tracked)?.map(|remote| remote.push)
        } else {
            None
        };
        if own_push_url != Some(false) {
            return Ok(false);
        }
        let fetched = git
            .git([OsStr::new("remote"), OsStr::new("get-url"), tracked])
            .run()?;
        if !fetched.success() {
            return Ok(false);
        }
        let formerly = OsString::from_vec(crate::strip_ascii(&fetched.stdout).to_vec());
        Ok(self.endpoint_at(&formerly, false)?.url == used)
    }
}

/// `os.path.join(base, value)`: `value` after `base` and a slash.
fn joined(mut base: Vec<u8>, value: &OsStr) -> OsString {
    if !base.is_empty() && !base.ends_with(b"/") {
        base.push(b'/');
    }
    base.extend_from_slice(value.as_bytes());
    OsString::from_vec(base)
}

/// Whether `value` has a remote name's shape, `[A-Za-z0-9._-]+`: a configured remote of that name
/// wins over a same-named path.
fn name_shaped(value: &OsStr) -> bool {
    !value.is_empty()
        && value
            .as_bytes()
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Whether git rewrites `url` before it reaches it: some `url.<base>.insteadOf` or
/// `url.<base>.pushInsteadOf` configured here is a prefix of it, as git matches them. Asked of the
/// rules, never by having git expand `url`, which it reads as a remote's name when it is one.
fn rewritten(git: &beadroll_git::Env, url: &OsStr) -> Result<bool, Error> {
    let rules = git
        .git([
            "config",
            "--null",
            "--get-regexp",
            r"^url\..+\.(insteadof|pushinsteadof)$",
        ])
        .run()?;
    Ok(rules.success()
        && rules.stdout.split(|b| *b == 0).any(|rule| {
            let prefix = rule
                .iter()
                .position(|b| *b == b'\n')
                .map(|at| &rule[at + 1..]);
            prefix.is_some_and(|prefix| url.as_bytes().starts_with(prefix))
        }))
}

/// A remote as git is configured with it.
struct Configured {
    /// Its URL, read as discovery reads origin's ([`configured_url`]).
    url: OsString,
    /// Whether that is a push URL of its own, rather than the URL git also fetches from.
    push: bool,
}

/// Remote `name` as git is configured with it, asked now; `None` when no remote of that name has a
/// URL. `name` has a remote name's shape, `[A-Za-z0-9._-]`, so only its dots need escaping in
/// git's pattern.
fn configured_remote(git: &beadroll_git::Env, name: &OsStr) -> Result<Option<Configured>, Error> {
    let escaped = name.to_string_lossy().replace('.', "\\.");
    let pattern = format!("^remote\\.{escaped}\\.(push)?url$");
    let listed = git
        .git(["config", "--get-regexp", pattern.as_str()])
        .run()?;
    if !listed.success() {
        return Ok(None);
    }
    let url = configured_url(&listed.stdout, name.as_bytes());
    let push = listed_url(&listed.stdout, name.as_bytes(), b"pushurl").is_some();
    Ok((!url.is_empty()).then(|| Configured {
        url: OsString::from_vec(url),
        push,
    }))
}

/// The URL remote `name` is configured with, from what `git config --get-regexp
/// ^remote\.NAME\.(push)?url$` printed (`listed`): where `git push NAME` goes, its first push URL,
/// else its first URL (the one git fetches from), as configured (`insteadOf` stays git's, not the
/// id's); empty when it has neither. Discovery reads origin with it, and a remote named by the
/// user is read the same way ([`Workspace::resolve_remote`], [`configured_remote`]).
pub(crate) fn configured_url(listed: &[u8], name: &[u8]) -> Vec<u8> {
    listed_url(listed, name, b"pushurl")
        .or_else(|| listed_url(listed, name, b"url"))
        .map(<[u8]>::to_vec)
        .unwrap_or_default()
}

/// The first value of `remote.NAME.KEY` that is not empty in what `git config --get-regexp`
/// printed (`listed`), as configured.
pub(crate) fn listed_url<'a>(listed: &'a [u8], name: &[u8], key: &[u8]) -> Option<&'a [u8]> {
    // git lowers the section and the key, never the remote's name
    let wanted = [&b"remote."[..], name, b".", key].concat();
    split_lines(listed).into_iter().find_map(|line| {
        let (listed_key, url) = match line.iter().position(|b| *b == b' ') {
            Some(at) => (&line[..at], &line[at + 1..]),
            None => (line, &b""[..]),
        };
        let url = beadroll_git::py_strip(url);
        (listed_key == wanted.as_slice() && !url.is_empty()).then_some(url)
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_split_as_python_splits_decoded_text() {
        assert_eq!(
            split_lines(b"a b\r\nc\xc2\x85d\xffe\n"),
            [&b"a b"[..], b"c", b"d\xffe"]
        );
    }

    #[test]
    fn a_remotes_configured_url_is_its_first_push_url_else_its_first_url() {
        let listed = b"remote.up.stream.url /fetch\nremote.up.stream.pushurl\n\
                       remote.up.stream.pushurl /push\nremote.up.stream.pushurl /second\n\
                       remote.Up.stream.pushurl /other\n";
        assert_eq!(configured_url(listed, b"up.stream"), b"/push");
        assert_eq!(configured_url(listed, b"Up.stream"), b"/other");
        assert_eq!(configured_url(listed, b"up"), b"");
        let fetched = b"remote.origin.url /fetch\nremote.origin.url /second\n";
        assert_eq!(configured_url(fetched, b"origin"), b"/fetch");
    }
}
