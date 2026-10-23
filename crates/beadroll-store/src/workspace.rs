//! One tracker's store on this machine, its checkout configuration and coordination remote.

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use beadroll_core::json::{self, Format, Object, Value};
use beadroll_core::metadata::Metadata;
use beadroll_core::text::repr_str;

use crate::error::Error;
use crate::fs::{Unreadable, read_json_keyed, write_atomic_keyed};
use crate::listing::Claims;
use crate::memo::FileKey;
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
    committed: Cell<bool>,
    pub(crate) claims: RefCell<Claims>,
    /// What `meta.json` held when this process last read or wrote it, by that file's identity.
    meta_read: RefCell<Option<(FileKey, MetaRead)>>,
}

/// `meta.json` as an object with a string remote, or what is wrong with it.
type MetaRead = Result<Arc<Metadata>, String>;

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
            committed: Cell::new(false),
            claims: RefCell::new(Claims::default()),
            meta_read: RefCell::new(None),
        })
    }

    /// Whether this process holds the store lock.
    pub fn holding(&self) -> bool {
        self.holding.get()
    }

    /// Records whether this process holds the store lock. A new locked or unlocked phase lists
    /// attempts afresh and stats each again: another command may write while a sync fetches
    /// outside the lock.
    pub fn set_holding(&self, holding: bool) {
        if self.holding.replace(holding) != holding {
            *self.claims.borrow_mut() = Claims::default();
        }
    }

    /// Forgets the attempt names listed in this phase, and the identity taken of `written`, the
    /// attempt file bead has just written: a claims directory that gains a file within one tick of
    /// the clock its times come from keeps its identity (Y01), so a write of an attempt has every
    /// directory listed again rather than trusting what the times say.
    pub fn forget_claims(&self, written: &Path) {
        self.claims.borrow_mut().forget(written);
    }

    /// Whether this process appended events the remote does not have yet.
    pub fn wrote(&self) -> bool {
        self.wrote.get()
    }

    /// Records whether this process appended events the remote does not have yet.
    pub fn set_wrote(&self, wrote: bool) {
        self.wrote.set(wrote);
    }

    /// Whether this workspace committed a local event or recognized this command's accepted
    /// ownership attempt, including a matching receipt from an earlier invocation.
    /// Publication may clear [`Self::wrote`], but cannot undo that effect of the command.
    pub fn committed(&self) -> bool {
        self.committed.get()
    }

    /// Records a successful durable write or this command's accepted receipt, retained for this
    /// workspace's lifetime so later bookkeeping cannot report the committed request as failed.
    pub fn record_commit(&self) {
        self.committed.set(true);
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
    pub fn meta(&self, strict: bool) -> Result<Arc<Object>, Error> {
        self.metadata(strict)
            .map(|meta| Arc::new((*meta).clone().into_object()))
    }

    /// Metadata with its published map kept as typed counts. Ordinary reads borrow only the
    /// fields they need; [`Self::meta`] retains the generic public API for compatibility.
    pub fn metadata(&self, strict: bool) -> Result<Arc<Metadata>, Error> {
        let path = self.dir.join("meta.json");
        let problem = match self.read_meta(&path) {
            Ok(meta) => return Ok(meta),
            Err(problem) => problem,
        };
        if !strict {
            return Ok(Arc::default());
        }
        Err(Error::io_message(format!(
            "{} cannot be used ({problem}): it records the coordination remote last used here; \
             repair it, or name the remote with --remote, which rewrites it",
            path.display()
        )))
    }

    /// `meta.json` at `path`, read once a command (roll-lqjj.30): again only when the file there
    /// no longer has the identity it had when this process last read or wrote it, since bead only
    /// ever replaces it whole. Its `published` map holds an entry for every journal file. The
    /// identity need not be settled: two replacements by others within one tick, the inode reused
    /// at the same length, go unseen, a residue `docs/REVIEWING.md` §2 weighs.
    fn read_meta(&self, path: &Path) -> MetaRead {
        if let Some(now) = FileKey::of(path)
            && let Some((key, read)) = &*self.meta_read.borrow()
            && *key == now
        {
            return read.clone();
        }
        let (read, key) = read_json_keyed(path, |bytes| {
            Metadata::read(bytes).map_err(|error| error.to_string())
        });
        let read = match read {
            Err(Unreadable::Missing) => Ok(Arc::default()),
            Ok(Some(meta)) if matches!(meta.value("remote"), None | Some(Value::Str(_))) => {
                Ok(Arc::new(meta))
            }
            Ok(_) => Err("not an object with a string remote".to_owned()),
            Err(Unreadable::Other(problem)) => Err(problem),
        };
        *self.meta_read.borrow_mut() = key.map(|key| (key, read.clone()));
        read
    }

    /// Records `values` in `meta.json`, in their order after the keys it has; empty values are
    /// skipped, and a `null` one forgets its key. A damaged file is kept for [`Workspace::remote`]
    /// to report, unless a remote is being recorded: that repairs it. Origin is never recorded: it
    /// is looked up each time.
    pub fn save_meta(&self, values: &[(&str, Value)]) -> Result<(), Error> {
        let mut meta = match self.metadata(true) {
            Ok(meta) => {
                // let go of, so that what was read is changed rather than copied
                self.meta_read.replace(None);
                Arc::unwrap_or_clone(meta).into_object()
            }
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
        let meta = Value::from(meta);
        let text =
            json::write(&meta, Format::INDENTED).map_err(|e| Error::io_message(e.to_string()))?;
        let path = self.dir.join("meta.json");
        let key = write_atomic_keyed(&path, text.as_bytes(), &self.settings.failpoints)?;
        if let (Some(key), Value::Object(meta)) = (key, meta) {
            *self.meta_read.borrow_mut() = Some((key, Ok(Arc::new(Metadata::from_object(*meta)))));
        }
        Ok(())
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

/// [`relative`], borrowed from `path` when it needs no copy: a path the store's own listings
/// made, which is the workspace directory, a separator and a name in UTF-8, is taken apart by its
/// bytes rather than its components, which took the unpublished count's loop over 92,000 attempt
/// files from 45 ms to 10 ms (roll-lqjj.47). Any other path is taken apart as `relative` always
/// did.
pub fn relative_name<'a>(ws: &Workspace, path: &'a Path) -> Cow<'a, str> {
    let dir = ws.dir.as_os_str().as_bytes();
    let bytes = path.as_os_str().as_bytes();
    // a name that starts with a separator or a dot, or ends in a separator or a `/.`, is one
    // `strip_prefix` would clean up
    if let Some(rest) = bytes
        .strip_prefix(dir)
        .and_then(|rest| rest.strip_prefix(b"/"))
        && rest
            .first()
            .is_some_and(|first| !matches!(first, b'/' | b'.'))
        && !rest.ends_with(b"/")
        && !rest.ends_with(b"/.")
        && let Ok(name) = std::str::from_utf8(rest)
    {
        return Cow::Borrowed(name);
    }
    path.strip_prefix(&ws.dir).unwrap_or(path).to_string_lossy()
}
