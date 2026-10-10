//! A workspace: one tracker's store on this machine, the configuration a checkout gives it, and the
//! coordination remote it uses (Python's `Workspace`).

use std::cell::Cell;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use beadroll_core::json::{self, Format, Object, Value};
use beadroll_core::text::repr_str;
use beadroll_git::{exists, is_local_path, normalize_remote, realpath, without_password};

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
    /// outside branches, tags and remote-tracking refs.
    pub fn ref_name(&self) -> Result<String, Error> {
        let name = match self.config.get("ref") {
            Some(value) if value.truthy() => beadroll_core::text::str_of(value),
            _ => DEFAULT_REF.to_owned(),
        };
        let bad = !name.starts_with("refs/")
            || ["refs/heads/", "refs/tags/", "refs/remotes/"]
                .iter()
                .any(|code| name.starts_with(code));
        if bad
            || !self
                .settings
                .git
                .git(["check-ref-format", name.as_str()])
                .run()?
                .success()
        {
            return Err(Error::Config(format!(
                "coordination ref {} must be a valid ref outside branches and tags",
                repr_str(&name)
            )));
        }
        Ok(name)
    }

    /// One remote spelling (a configured remote's name, a URL, a path) reduced to the one bead
    /// uses, as git would read it: a configured remote wins over a same-named path, a stored
    /// relative path is read from the top of the checkout whatever the cwd, a path typed just now
    /// (`typed`) from the cwd. A password is dropped.
    pub fn resolve_remote(&self, value: &OsStr, typed: bool) -> Result<OsString, Error> {
        let git = &self.settings.git;
        let mut value = value.to_owned();
        let mut typed = typed;
        let name_shaped = !value.is_empty()
            && value
                .as_bytes()
                .iter()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
        if name_shaped {
            let named = git
                .git([OsStr::new("remote"), OsStr::new("get-url"), &value])
                .run()?;
            if named.success() {
                value = OsString::from_vec(crate::strip_ascii(&named.stdout).to_vec());
                typed = false;
            }
        }
        let value = without_password(&value).0;
        let mut value = value;
        if !typed && is_local_path(&value) && !value.as_bytes().starts_with(b"/") {
            let top = git.git(["rev-parse", "--show-toplevel"]).run()?;
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
            && self.resolve_remote(tracked, false)? != *used
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
