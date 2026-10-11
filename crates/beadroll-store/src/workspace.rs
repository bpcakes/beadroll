//! A workspace: one tracker's store on this machine, the configuration a checkout gives it, and the
//! coordination remote it uses (Python's `Workspace`).

use std::cell::Cell;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use beadroll_core::json::{self, Format, Object, Value};
use beadroll_core::text::repr_str;

use crate::error::Error;
use crate::fs::{Unreadable, read_json_text, write_atomic};
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
    /// The checkout's origin URL, as configured.
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
            && self.is_origin_as_reached(remote, &meta)?
        {
            meta.shift_remove("remote");
            meta.shift_remove("remote_push_url");
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
    pub fn note_sync(&self, remote: &Endpoint, values: &[(&str, Value)]) {
        let mut all: Vec<(&str, Value)> = values.to_vec();
        all.extend(remembered(remote));
        all.push(("last_sync", Value::from(stamp())));
        let _ = self.save_meta(&all);
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
}

/// What `meta.json` records of `remote`, the endpoint a store went through: its URL, and beside it
/// the same URL when it was a remote's own push URL, which the next command chosen by it, with no
/// name to look up, pushes to as that remote's (`null` forgets an earlier one).
pub fn remembered(remote: &Endpoint) -> [(&'static str, Value); 2] {
    let url = Value::from(remote.url.to_string_lossy().into_owned());
    let pushed = if remote.own_push_url {
        url.clone()
    } else {
        Value::Null
    };
    [("remote", url), ("remote_push_url", pushed)]
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
