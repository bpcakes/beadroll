//! One tracker's store on this machine, its checkout configuration and coordination remote.

use std::cell::{Cell, RefCell};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use beadroll_core::json::{self, Format, Object, Value};
use beadroll_core::text::repr_str;

use crate::error::Error;
use crate::fs::{Unreadable, read_json_text, write_atomic};
use crate::listing::Claims;
use crate::settings::Settings;

mod remote;

pub use remote::Endpoint;
pub(crate) use remote::{configured_url, listed_url};

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
    /// The checkout's origin URL, as configured: where `git push origin` goes (its first push URL,
    /// else its first URL), a relative path as written, which git reads from the top of the
    /// checkout, where bead runs it.
    pub origin: Option<OsString>,
    /// Whether that is origin's own push URL (`remote.origin.pushurl`), which git pushes to as
    /// configured, without `pushInsteadOf`.
    pub origin_push: bool,
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
    pub(crate) claims: RefCell<Claims>,
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
            origin_push: false,
            config,
            derived: None,
            events_dir: dir.join("events"),
            dir,
            settings,
            holding: Cell::new(false),
            wrote: Cell::new(false),
            claims: RefCell::new(Claims::default()),
        })
    }

    /// Whether this process holds the store lock.
    pub fn holding(&self) -> bool {
        self.holding.get()
    }

    /// Records whether this process holds the store lock. A new locked or unlocked phase lists
    /// attempts afresh: another command may write while a sync fetches outside the lock.
    pub fn set_holding(&self, holding: bool) {
        if self.holding.replace(holding) != holding {
            *self.claims.borrow_mut() = Claims::default();
        }
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
                return Ok(*meta);
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
    /// skipped, and a `null` one forgets its key. A damaged file is kept for [`Workspace::remote`]
    /// to report, unless a remote is being recorded: that repairs it. Origin is never recorded: it
    /// is looked up each time.
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
            } else if matches!(value, Value::Null) {
                meta.shift_remove(*key);
            }
        }
        if let Some(Value::Str(remote)) = meta.get("remote")
            && self.is_origin_chosen(OsStr::new(remote))?
        {
            meta.shift_remove("remote");
        }
        let text = json::write(&Value::from(meta), Format::INDENTED)
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
    pub fn note_sync(&self, remote: &Endpoint, values: &[(&str, Value)]) {
        let mut all: Vec<(&str, Value)> = values.to_vec();
        all.push(("remote", remembered(remote)));
        all.push(("last_sync", Value::from(stamp())));
        let _ = self.save_meta(&all);
    }

    /// The coordination ref: the tracked one, else `refs/beads/journal`. It must be a valid ref
    /// outside branches, tags and remote-tracking refs: `git check-ref-format` decides, except for
    /// the default, which is one and needs no git to say so.
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
}

/// What `meta.json` records of `remote`, the remote a store went through: as it was chosen, a
/// remote's name or a URL ([`Endpoint::chosen`]), which the next command reads again as it reads a
/// remote given to it (a name is looked up then).
pub fn remembered(remote: &Endpoint) -> Value {
    Value::from(remote.chosen().to_string_lossy().into_owned())
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

/// The real current time in UTC to the second for last_sync, unaffected by event-clock shifts.
pub fn stamp() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    beadroll_core::time::utc_stamp(seconds)
}

/// The path relative to the workspace directory.
pub fn relative(ws: &Workspace, path: &Path) -> String {
    path.strip_prefix(&ws.dir)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}
