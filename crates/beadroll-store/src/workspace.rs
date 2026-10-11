//! A workspace: one tracker's store on this machine, the configuration a checkout gives it, and the
//! coordination remote it uses (Python's `Workspace`).

use std::cell::Cell;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use beadroll_core::json::{self, Format, Object, Value};
use beadroll_core::text::repr_str;
use beadroll_git::{Fact, exists, is_local_path, normalize_remote, realpath, without_password};

use crate::error::Error;
use crate::fs::{Unreadable, read_json_text, write_atomic};
use crate::settings::Settings;

/// The coordination ref when the tracked configuration names none.
pub const DEFAULT_REF: &str = "refs/beads/journal";
/// Where `sync.git` keeps what it last fetched or published.
pub const REMOTE_REF: &str = "refs/beads/remote";

/// One tracker's store on this machine.
#[derive(Debug)]
pub struct Workspace {
    /// Its opaque id: the store's directory name.
    pub id: String,
    /// Its display name: the origin's last path component, else the checkout's directory.
    pub name: Option<OsString>,
    /// The checkout's origin URL, as configured.
    pub origin: Option<OsString>,
    /// The tracked configuration (`.beadroll.json`): what it says, unknown keys included.
    pub config: Object,
    /// The URL-derived id, when that is where this id came from (`adopt_published`).
    pub derived: Option<String>,
    /// The store's directory, `$BEAD_HOME/<id>`.
    pub dir: PathBuf,
    /// Its journals, `<dir>/events`.
    pub events_dir: PathBuf,
    /// The settings it was found with.
    pub settings: Arc<Settings>,
    holding: Cell<bool>,
    wrote: Cell<bool>,
}

impl Workspace {
    /// The workspace `id` (an opaque token, never a path) of a checkout named `name` with `origin`
    /// and tracked configuration `config`.
    pub fn new(
        settings: Arc<Settings>,
        id: &str,
        name: Option<OsString>,
        origin: Option<OsString>,
        config: Object,
    ) -> Result<Workspace, Error> {
        if !opaque_token(id) {
            return Err(Error::Config(format!(
                "workspace id must be an opaque token, not a path: {}",
                repr_str(id)
            )));
        }
        let dir = settings.home()?.join(id);
        Ok(Workspace {
            id: id.to_owned(),
            name,
            origin,
            config,
            derived: None,
            events_dir: dir.join("events"),
            dir,
            settings,
            holding: Cell::new(false),
            wrote: Cell::new(false),
        })
    }

    /// Whether this process holds the store lock.
    pub fn holding(&self) -> bool {
        self.holding.get()
    }

    /// Records whether this process holds the store lock.
    pub fn set_holding(&self, holding: bool) {
        self.holding.set(holding);
    }

    /// Whether this process appended events the remote does not have yet.
    pub fn wrote(&self) -> bool {
        self.wrote.get()
    }

    /// Records whether this process appended events the remote does not have yet.
    pub fn set_wrote(&self, wrote: bool) {
        self.wrote.set(wrote);
    }

    /// A tracked configuration value that is a string.
    pub fn config_str(&self, key: &str) -> Option<&str> {
        match self.config.get(key) {
            Some(Value::Str(value)) => Some(value),
            _ => None,
        }
    }

    /// What this machine last used (`meta.json`). A missing file is empty. With `strict` (for
    /// [`Workspace::remote`]), the file may be the only record of the coordination remote: one
    /// that cannot be read or parsed, or whose remote is not a string, is an error, never a
    /// silent switch to origin.
    pub fn meta(&self, strict: bool) -> Result<Object, Error> {
        let path = self.dir.join("meta.json");
        let problem = match read_json_text(&path) {
            Err(Unreadable::Missing) => return Ok(Object::new()),
            Ok(Value::Object(meta)) if matches!(meta.get("remote"), None | Some(Value::Str(_))) => {
                return Ok(meta);
            }
            Ok(_) => "not an object with a string remote".to_owned(),
            Err(Unreadable::Other(problem)) => problem,
        };
        if !strict {
            return Ok(Object::new());
        }
        Err(Error::io_message(format!(
            "{} cannot be used ({problem}): it records the coordination remote last used here; \
             repair it, or name the remote with --remote, which rewrites it",
            path.display()
        )))
    }

    /// Records `values` in `meta.json`, in their order after the keys it has; empty values are
    /// skipped. A damaged file is kept for [`Workspace::remote`] to report, unless a remote is
    /// being recorded: that repairs it. Origin is never recorded: it is looked up each time.
    pub fn save_meta(&self, values: &[(&str, Value)]) -> Result<(), Error> {
        let mut meta = match self.meta(true) {
            Ok(meta) => meta,
            Err(_) => {
                let remote = values.iter().find(|(key, _)| *key == "remote");
                if !remote.is_some_and(|(_, value)| value.truthy()) {
                    return Ok(());
                }
                Object::new()
            }
        };
        for (key, value) in values {
            if value.truthy() {
                meta.insert((*key).to_owned(), value.clone());
            }
        }
        if let Some(Value::Str(remote)) = meta.get("remote")
            && self.is_origin(OsStr::new(remote))
        {
            meta.shift_remove("remote");
        }
        let text = json::write(&Value::Object(meta), Format::INDENTED)
            .map_err(|e| Error::io_message(e.to_string()))?;
        write_atomic(
            &self.dir.join("meta.json"),
            text.as_bytes(),
            true,
            &self.settings.failpoints,
        )
    }

    /// Bookkeeping once the remote has accepted. It never turns a command that took effect into a
    /// failure: a full volume or a damaged `meta.json` only leaves `last_sync` behind.
    pub fn note_sync(&self, remote: &OsStr, values: &[(&str, Value)]) {
        let mut all: Vec<(&str, Value)> = values.to_vec();
        all.push(("remote", Value::from(remote.to_string_lossy().into_owned())));
        all.push(("last_sync", Value::from(stamp())));
        let _ = self.save_meta(&all);
    }

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

    /// The coordination ref: the tracked one, else `refs/beads/journal`. It must be a valid ref
    /// outside branches, tags and remote-tracking refs: `git check-ref-format` decides, except for
    /// the default, which is one (`docs/RUST-PORT.md` Appendix B).
    pub fn ref_name(&self) -> Result<String, Error> {
        let name = match self.config.get("ref") {
            Some(value) if value.truthy() => beadroll_core::text::str_of(value),
            _ => DEFAULT_REF.to_owned(),
        };
        let bad = !name.starts_with("refs/")
            || ["refs/heads/", "refs/tags/", "refs/remotes/"]
                .iter()
                .any(|code| name.starts_with(code));
        let valid = || -> Result<bool, Error> {
            if name == DEFAULT_REF {
                return Ok(true);
            }
            let checked = self.settings.git.git(["check-ref-format", name.as_str()]);
            Ok(checked.run()?.success())
        };
        if bad || !valid()? {
            return Err(Error::Config(format!(
                "coordination ref {} must be a valid ref outside branches and tags",
                repr_str(&name)
            )));
        }
        Ok(name)
    }

    /// One remote spelling (a configured remote's name, a URL, a path) reduced to the one bead
    /// uses, as git would read it: a configured remote wins over a same-named path, a stored
    /// relative path is read from the top of the checkout whatever the cwd, unless git rewrites it
    /// (`insteadOf`), a path typed just now (`typed`) from the cwd. A password is dropped.
    pub fn resolve_remote(&self, value: &OsStr, typed: bool) -> Result<OsString, Error> {
        let git = &self.settings.git;
        let mut value = value.to_owned();
        let mut typed = typed;
        if name_shaped(&value)
            && let Some(Configured { url, .. }) = configured_remote(git, &value)?
        {
            // where `git push NAME` goes, as configured, as discovery reads origin (G22): git
            // applies `insteadOf` and `pushInsteadOf` itself when bead hands it that URL
            value = url;
            typed = false;
        }
        let value = without_password(&value).0;
        let mut value = value;
        if !typed && is_local_path(&value) && !value.as_bytes().starts_with(b"/") {
            if rewritten(git, &value)? {
                // a path in spelling only: git makes a URL of it, which it could no longer do to
                // the path bead would make of it
                return Ok(value);
            }
            let top = self.settings.fact(Fact::Toplevel)?;
            if top.success() {
                let top = crate::strip_ascii(&top.stdout);
                // os.path.join(top, value)
                let mut joined = top.to_vec();
                if !joined.is_empty() && !joined.ends_with(b"/") {
                    joined.push(b'/');
                }
                joined.extend_from_slice(value.as_bytes());
                value = OsString::from_vec(joined);
            }
        }
        let cwd = &self.settings.cwd;
        Ok(if exists(&value, cwd) {
            realpath(&value, cwd).into_os_string()
        } else {
            value
        })
    }

    /// The one coordination endpoint: `override` (`--remote`), else `BEAD_REMOTE`, the tracked
    /// configuration, the remote last named here, then origin. A tracked configuration seeds a new
    /// store's remote, but never silently moves an existing store's.
    pub fn remote(&self, override_: Option<&OsStr>) -> Result<Option<OsString>, Error> {
        let typed = override_
            .filter(|value| !value.is_empty())
            .or_else(|| self.settings.var("BEAD_REMOTE"));
        if let Some(typed) = typed {
            return self.resolve_remote(typed, true).map(Some);
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
        let value = tracked
            .or(used)
            .or_else(|| self.origin.clone().filter(|o| !o.is_empty()));
        match value {
            None => Ok(None),
            Some(value) => self.resolve_remote(&value, false).map(Some),
        }
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
        Ok(self.resolve_remote(&formerly, false)? == used)
    }
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

/// Whether git rewrites `url` (`url.<base>.insteadOf`) before it reaches it: what `git ls-remote
/// --get-url` makes of it, without asking any remote, is something else.
fn rewritten(git: &beadroll_git::Env, url: &OsStr) -> Result<bool, Error> {
    if url.as_bytes().starts_with(b"-") {
        return Ok(false);
    }
    let expanded = git
        .git([OsStr::new("ls-remote"), OsStr::new("--get-url"), url])
        .run()?;
    Ok(expanded.success() && crate::strip_ascii(&expanded.stdout) != url.as_bytes())
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
fn listed_url<'a>(listed: &'a [u8], name: &[u8], key: &[u8]) -> Option<&'a [u8]> {
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

/// `[A-Za-z0-9][A-Za-z0-9._-]{0,99}`, whole.
fn opaque_token(id: &str) -> bool {
    let bytes = id.as_bytes();
    (1..=100).contains(&bytes.len())
        && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Now, as `last_sync` records it: UTC to the second (Python's `stamp()`). The real clock, never a
/// shifted one.
pub fn stamp() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    beadroll_core::time::utc_stamp(seconds)
}

/// `<path>` relative to the workspace's directory, as Python's `str(p.relative_to(ws.dir))`.
pub fn relative(ws: &Workspace, path: &Path) -> String {
    path.strip_prefix(&ws.dir)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
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
