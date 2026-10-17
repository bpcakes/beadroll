//! Local recap bookmarks and store discovery, independent of any checkout or remote.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::Path;
use std::sync::Arc;

use beadroll_core::id::EventId;
use beadroll_core::json::{self, Format, Object, Value};
use beadroll_git::IoError;

use crate::{Error, Settings, Unreadable, Workspace, dir_entries, private_dir, read_json_text};

/// Every local store under `BEAD_HOME`, in workspace-id order. An `events` directory identifies
/// a store; other files and directories in the home are not stores. No checkout is consulted.
pub fn recap_workspaces(settings: &Arc<Settings>) -> Result<Vec<Workspace>, Error> {
    let mut stores = Vec::new();
    for path in dir_entries(&settings.home()?)? {
        let meta = match std::fs::metadata(&path) {
            Ok(meta) => meta,
            // Atomic writes elsewhere in the home may retire a temporary file after listing.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(IoError::at(&error, &path).into()),
        };
        if !meta.is_dir() {
            continue;
        }
        let events = path.join("events");
        match std::fs::metadata(&events) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => {
                return Err(Error::io_message(format!(
                    "{} is not a journal directory",
                    events.display()
                )));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(IoError::at(&error, &events).into()),
        }
        let id = path.file_name().and_then(|n| n.to_str()).ok_or_else(|| {
            Error::Config(format!("{} has no UTF-8 workspace id", path.display()))
        })?;
        stores.push(Workspace::new(
            Arc::clone(settings),
            id,
            None,
            None,
            Object::new(),
        )?);
    }
    Ok(stores)
}

/// The events already delivered by recap, per workspace. An advancing reader holds a separate
/// home-wide lock until it saves or drops this value; journal writers never take this lock.
/// Event ids, rather than a timestamp or per-machine maximum, keep late arrivals visible.
/// Its filenames begin with a dot, which no workspace id can, so an existing store cannot
/// collide with either file.
#[derive(Debug)]
pub struct RecapCursor {
    settings: Arc<Settings>,
    seen: BTreeMap<String, BTreeSet<EventId>>,
    lock: Option<File>,
    changed: bool,
}

impl RecapCursor {
    /// Reads the bookmark, taking its lock first when `advance`. A preview creates nothing.
    /// An unreadable or unrecognized bookmark is an error, never an empty replacement.
    pub fn open(settings: Arc<Settings>, advance: bool) -> Result<Self, Error> {
        let home = settings.home()?;
        let lock = if advance {
            private_dir(&home)?;
            let path = home.join(".recap.lock");
            let lock = OpenOptions::new()
                .read(true)
                .append(true)
                .create(true)
                .mode(0o600)
                .open(&path)
                .map_err(|e| IoError::at(&e, &path))?;
            lock.lock().map_err(|e| IoError::at(&e, &path))?;
            Some(lock)
        } else {
            None
        };
        let seen = read_cursor(&home.join(".recap.json"))?;
        Ok(Self {
            settings,
            seen,
            lock,
            changed: false,
        })
    }

    /// Whether a previous successful recap delivered this event of this workspace.
    pub fn contains(&self, workspace: &str, event: EventId) -> bool {
        self.seen
            .get(workspace)
            .is_some_and(|seen| seen.contains(&event))
    }

    /// Remembers events in this answer. Nothing is written until [`Self::save`].
    pub fn record(&mut self, workspace: &str, events: impl IntoIterator<Item = EventId>) {
        if self.lock.is_none() {
            return;
        }
        let seen = self.seen.entry(workspace.to_owned()).or_default();
        for event in events {
            self.changed |= seen.insert(event);
        }
    }

    /// Saves the bookmark atomically, after the caller has delivered and flushed its answer.
    /// Dropping it without saving leaves the previous bookmark intact. A preview never writes.
    pub fn save(self) -> Result<(), Error> {
        if !self.changed || self.lock.is_none() {
            return Ok(());
        }
        let workspaces: Object = self
            .seen
            .iter()
            .map(|(workspace, events)| {
                let ids = events
                    .iter()
                    .map(|id| Value::from(id.to_string()))
                    .collect();
                (workspace.clone(), Value::Array(ids))
            })
            .collect();
        let mut value = Object::new();
        value.insert("version".into(), Value::from(1));
        value.insert("workspaces".into(), Value::from(workspaces));
        let bytes = json::write(&Value::from(value), Format::COMPACT)
            .map_err(|e| Error::Validation(e.to_string()))?;
        let path = self.settings.home()?.join(".recap.json");
        crate::write_atomic(&path, bytes.as_bytes(), true, &self.settings.failpoints).map_err(
            |error| match error {
                Error::Io(mut error) if error.path.is_none() => {
                    error.path = Some(path);
                    Error::Io(error)
                }
                error => error,
            },
        )
    }
}

fn read_cursor(path: &Path) -> Result<BTreeMap<String, BTreeSet<EventId>>, Error> {
    let bad = |why: &str| {
        Error::io_message(format!(
            "{} cannot be used ({why}); repair it, or remove it to show all activity again",
            path.display()
        ))
    };
    let value = match read_json_text(path) {
        Ok(value) => value,
        Err(Unreadable::Missing) => return Ok(BTreeMap::new()),
        Err(Unreadable::Other(why)) => return Err(bad(&why)),
    };
    let object = value
        .as_object()
        .ok_or_else(|| bad("not a cursor object"))?;
    if object.len() != 2 || object.get("version") != Some(&Value::from(1)) {
        return Err(bad("unsupported cursor version or members"));
    }
    let workspaces = object
        .get("workspaces")
        .and_then(Value::as_object)
        .ok_or_else(|| bad("workspaces is not an object"))?;
    let mut seen = BTreeMap::new();
    for (workspace, events) in workspaces {
        let events = events
            .as_array()
            .ok_or_else(|| bad("events is not an array"))?;
        let ids = events
            .iter()
            .map(|event| {
                event
                    .as_str()
                    .and_then(EventId::parse)
                    .ok_or_else(|| bad("event is not a UUID"))
            })
            .collect::<Result<BTreeSet<_>, _>>()?;
        seen.insert(workspace.clone(), ids);
    }
    Ok(seen)
}
