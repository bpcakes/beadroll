//! Remote resolution: what a remote given to bead (a configured remote's name, a URL, a path)
//! means, and the one coordination remote a store goes through.
//!
//! A remote is kept as it was chosen: the name of a remote the checkout configures (origin when
//! none is chosen), or a URL or a path as given. That choice is what a store remembers and what is
//! compared; a name is read for its URL only to reach the remote, where git applies its own rules
//! to it, as for `git fetch NAME` and `git push NAME` (the link defines a remote at that URL, its
//! own push URL as its push URL).

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};

use beadroll_core::json::Value;
use beadroll_git::{Fact, is_local_path, normalize_remote, realpath, without_password};

use super::Workspace;
use crate::MetadataRead;
use crate::error::Error;

/// Where a remote spelling came from, for resolving relative paths.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteSource {
    /// Remembered.
    Remembered,
    /// Typed.
    Typed,
}

/// The coordination remote as it was chosen ([`Workspace::endpoint`]) and as git is given it: the
/// URL a link defines a remote at, whether that is the remote's own push URL (which git pushes to
/// as configured, without `pushInsteadOf`), and the name it was chosen by, if it was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    /// The URL, as bead hands it to git.
    pub url: OsString,
    /// Whether it is the remote's own push URL.
    pub own_push_url: bool,
    /// The name of the remote it was chosen by, when it was (`origin` for origin).
    pub name: Option<OsString>,
}

impl Endpoint {
    /// The endpoint at `url`, given by its URL alone: git applies its rules to it as to any URL.
    pub fn at(url: impl Into<OsString>) -> Endpoint {
        Endpoint {
            url: url.into(),
            own_push_url: false,
            name: None,
        }
    }

    /// The remote as it was chosen: its name, else its URL. What a store remembers, what is
    /// compared, and how a command gives the remote again.
    pub fn chosen(&self) -> &OsStr {
        self.name.as_deref().unwrap_or(&self.url)
    }
}

impl Workspace {
    /// The remote last named here, as it was chosen (its name or its URL). Origin is never
    /// remembered (older versions saved its URL too): it is looked up each time, so `git remote
    /// set-url origin` is followed.
    pub fn remembered_remote(&self) -> Result<Option<OsString>, Error> {
        let meta = self.metadata(MetadataRead::Strict)?;
        Ok(match meta.value("remote") {
            Some(Value::Str(remote)) if !self.is_origin_chosen(OsStr::new(remote))? => {
                Some(OsString::from(remote))
            }
            _ => None,
        })
    }

    /// Whether `chosen`, a remote as it was chosen, is origin: its name, or, as an older version
    /// saved it, origin's URL: the same spelling, or, for two URLs that are not local paths and
    /// that no rule rewrites, the same repository however spelled (ssh, https, `.git`), which a
    /// forge serves alike and the URL-derived id already takes for one; two local paths are origin
    /// only spelled alike, since `x` and `x.git` are two repositories.
    pub(crate) fn is_origin_chosen(&self, chosen: &OsStr) -> Result<bool, Error> {
        let Some(origin) = self.origin.clone().filter(|origin| !origin.is_empty()) else {
            return Ok(false);
        };
        if chosen == "origin" {
            return Ok(true);
        }
        if chosen.is_empty() || self.names_remote(chosen)? {
            return Ok(false);
        }
        if self.is_origin_url(chosen, RemoteSource::Remembered) {
            return Ok(true);
        }
        let url = without_password(chosen).0;
        let cwd = &self.settings.cwd;
        Ok(!is_local_path(&url)
            && !is_local_path(&origin)
            && normalize_remote(&url, cwd) == normalize_remote(&origin, cwd)
            && !self.rewrites(&url)?
            && !self.rewrites(&origin)?)
    }

    /// Whether `value`, a URL or a path as given, is origin's own URL: spelled as origin is
    /// configured, a password dropped from each, which git reaches alike, rules and all; never two
    /// spellings taken for one place. A relative path typed just now (`typed`) is read from where
    /// it was typed, a path of its own.
    pub fn is_origin_url(&self, value: &OsStr, typed: RemoteSource) -> bool {
        let typed = matches!(typed, RemoteSource::Typed);
        let Some(origin) = self.origin.as_deref().filter(|origin| !origin.is_empty()) else {
            return false;
        };
        let relative = is_local_path(value) && !value.as_bytes().starts_with(b"/");
        !(typed && relative) && without_password(value).0 == without_password(origin).0
    }

    /// One remote spelling (a configured remote's name, a URL, a path) reduced to the URL bead
    /// uses ([`Workspace::resolve_endpoint`]).
    pub fn resolve_remote(
        &self,
        value: &OsStr,
        remote_source: RemoteSource,
    ) -> Result<OsString, Error> {
        Ok(self.resolve_endpoint(value, remote_source)?.url)
    }

    /// One remote spelling as bead reads it: a configured remote's name wins over a same-named
    /// path, and is that remote, reached at the URL it is configured with for `git push NAME` (its
    /// first push URL, else its first URL, as written: git rewrites it, and reads a relative path
    /// from the top of the checkout, where bead runs it); anything else is a URL or a path
    /// ([`Workspace::endpoint_at`]), and origin's own URL so spelled is origin, reached as origin
    /// is, its own push URL included. A password is dropped.
    pub fn resolve_endpoint(
        &self,
        value: &OsStr,
        remote_source: RemoteSource,
    ) -> Result<Endpoint, Error> {
        if name_shaped(value)
            && let Some(Configured { url, push }) = configured_remote(&self.settings.git, value)?
        {
            return Ok(Endpoint {
                url: without_password(&url).0,
                own_push_url: push,
                name: Some(value.to_owned()),
            });
        }
        if self.is_origin_url(value, remote_source)
            && let Some(origin) = self.origin_endpoint()
        {
            return Ok(origin);
        }
        self.endpoint_at(value, remote_source)
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

    /// `value` as a URL or a path, never a remote's name: a relative path read from the top of the
    /// checkout when it is stored, from the cwd when it was typed just now (`typed`), and a path
    /// through its symbolic links, whether it exists or not; except a spelling git rewrites
    /// (`insteadOf`), kept as written for git to rewrite. A password is dropped.
    fn endpoint_at(&self, value: &OsStr, typed: RemoteSource) -> Result<Endpoint, Error> {
        let typed = matches!(typed, RemoteSource::Typed);
        let git = &self.settings.git;
        let mut value = without_password(value).0;
        if is_local_path(&value) {
            if rewritten(git, &value)? {
                // a path in spelling only: git makes a URL of it, which it could no longer do to
                // the path bead would make of it
                return Ok(Endpoint::at(value));
            }
            if !value.as_bytes().starts_with(b"/") {
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
            let real = realpath(&value, &self.settings.cwd).into_os_string();
            // the whole spelling a rule may rewrite, which the resolved one no longer starts with
            if real != value && rewritten(git, &value)? {
                return Ok(Endpoint::at(value));
            }
            value = real;
        }
        Ok(Endpoint::at(value))
    }

    /// The URL of the one coordination remote ([`Workspace::endpoint`]).
    pub fn remote(&self, override_: Option<&OsStr>) -> Result<Option<OsString>, Error> {
        Ok(self.endpoint(override_)?.map(|endpoint| endpoint.url))
    }

    /// The one coordination remote: `override` (`--remote`), else `BEAD_REMOTE`, the tracked
    /// configuration, the remote last named here, then origin, each read as
    /// [`Workspace::resolve_endpoint`] reads it. A tracked configuration seeds a new store's
    /// remote, but never silently moves an existing store's ([`Workspace::same_choice`]).
    pub fn endpoint(&self, override_: Option<&OsStr>) -> Result<Option<Endpoint>, Error> {
        let typed = override_
            .filter(|value| !value.is_empty())
            .or_else(|| self.settings.var("BEAD_REMOTE"));
        if let Some(typed) = typed {
            return self.resolve_endpoint(typed, RemoteSource::Typed).map(Some);
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
            && !self.same_choice(tracked, used)?
        {
            return Err(Error::Config(format!(
                ".beadroll.json names coordination remote {}, but this tracker already coordinates \
                 through {}; change it deliberately with `bead sync --remote` or `bead init \
                 --tracked --remote`",
                without_password(tracked).0.to_string_lossy(),
                used.to_string_lossy()
            )));
        }
        match tracked.or(used) {
            Some(chosen) => self
                .resolve_endpoint(&chosen, RemoteSource::Remembered)
                .map(Some),
            None => Ok(self.origin_endpoint()),
        }
    }

    /// Origin as it is reached when it is the coordination remote, as `git push origin` reaches it:
    /// its URL as configured ([`Workspace::origin`]), its own push URL or not, the same endpoint
    /// as the name `origin` given ([`Workspace::resolve_endpoint`]); `None` when the checkout has
    /// none.
    pub fn origin_endpoint(&self) -> Option<Endpoint> {
        let origin = self.origin.clone().filter(|origin| !origin.is_empty())?;
        Some(Endpoint {
            url: without_password(&origin).0,
            own_push_url: self.origin_push,
            name: Some(OsString::from("origin")),
        })
    }

    /// Whether the tracked remote `tracked` is `used`, the remote this store last went through,
    /// as they were chosen: the same name, or the same URL. A name and a URL are the same only as
    /// an older version saved a name, as git expanded it ([`Workspace::saved_for`]).
    fn same_choice(&self, tracked: &OsStr, used: &OsStr) -> Result<bool, Error> {
        let tracked = self.resolve_endpoint(tracked, RemoteSource::Remembered)?;
        let used = self.resolve_endpoint(used, RemoteSource::Remembered)?;
        match (&tracked.name, &used.name) {
            (Some(tracked), Some(used)) => Ok(tracked == used),
            (None, None) => Ok(tracked.url == used.url),
            (Some(name), None) => self.saved_for(name, &used.url),
            (None, Some(_)) => Ok(false),
        }
    }

    /// Whether `saved`, a URL an older version remembered for remote `name` (`git remote get-url
    /// NAME`, resolved, its password dropped), is where git takes that remote: git fetches the
    /// name there and pushes it there, and takes `saved` itself there too, each answer git's own
    /// (`git remote get-url`, with and without `--push`, and `git ls-remote --get-url`, every rule
    /// applied once), read as the link reads it, a password dropped as from every URL.
    fn saved_for(&self, name: &OsStr, saved: &OsStr) -> Result<bool, Error> {
        if saved.as_bytes().starts_with(b"-") {
            return Ok(false);
        }
        let git = &self.settings.git;
        let ask = |args: &[&OsStr]| -> Result<Option<OsString>, Error> {
            let asked = git.git(args).run()?;
            let url = OsString::from_vec(crate::strip_ascii(&asked.stdout).to_vec());
            Ok(asked.success().then_some(url))
        };
        let (remote, get_url, push) = (OsStr::new("remote"), OsStr::new("get-url"), "--push");
        let answers = [
            ask(&[remote, get_url, name])?,
            ask(&[remote, get_url, OsStr::new(push), name])?,
            ask(&[OsStr::new("ls-remote"), OsStr::new("--get-url"), saved])?,
        ];
        let mut places = Vec::new();
        for answer in answers {
            let Some(url) = answer else {
                return Ok(false);
            };
            places.push(self.place(&without_password(&url).0)?);
        }
        Ok(places.windows(2).all(|pair| pair[0] == pair[1]))
    }

    /// `url`, which git answered with every rule applied, where the link reaches it: a relative
    /// path from the top of the checkout, through its symbolic links; any other URL as it is.
    fn place(&self, url: &OsStr) -> Result<OsString, Error> {
        if !is_local_path(url) {
            return Ok(url.to_owned());
        }
        let mut path = url.to_owned();
        if !url.as_bytes().starts_with(b"/") {
            let top = self.settings.fact(Fact::Toplevel)?;
            if top.success() {
                path = joined(crate::strip_ascii(&top.stdout).to_vec(), url);
            }
        }
        Ok(realpath(&path, &self.settings.cwd).into_os_string())
    }
}

/// Appends `value` to `base`, inserting a slash if needed.
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
    let url = url.as_bytes();
    Ok(rules.success()
        && rules.stdout.split(|b| *b == 0).any(|rule| {
            let prefix = rule
                .iter()
                .position(|b| *b == b'\n')
                .map(|at| &rule[at + 1..]);
            prefix.is_some_and(|prefix| url.starts_with(prefix))
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
/// user is read the same way ([`Workspace::resolve_endpoint`], [`configured_remote`]).
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
        let url = beadroll_git::strip_text_bytes(url);
        (listed_key == wanted.as_slice() && !url.is_empty()).then_some(url)
    })
}

/// Splits OS bytes at text line boundaries. ASCII separators and multibyte separator encodings
/// cannot occur inside another valid character, so invalid bytes can remain untouched.
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
    fn lines_split_at_unicode_boundaries() {
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

    #[test]
    fn a_remote_is_remembered_and_compared_as_it_was_chosen() {
        let named = Endpoint {
            url: OsString::from("git@host:o/r.git"),
            own_push_url: true,
            name: Some(OsString::from("tracker")),
        };
        assert_eq!(named.chosen(), "tracker");
        assert_eq!(Endpoint::at("/srv/r.git").chosen(), "/srv/r.git");
    }
}
