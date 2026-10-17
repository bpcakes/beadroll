//! What `bead recap` reads and keeps on this machine (roll-dih8.2): every tracker's store under
//! `BEAD_HOME`, found without a checkout, and the cursor, what it has shown of each.

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::ErrorKind;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::Path;
use std::sync::Arc;

use beadroll_core::id::EventId;
use beadroll_core::json::{self, Format, Object, Value};
use beadroll_core::text::printed_text;
use beadroll_git::IoError;

use crate::{Error, Settings, Unreadable, Workspace, dir_entries, private_dir, read_json_text};

/// The cursor's file, in `BEAD_HOME`. Its name starts with a dot, which no workspace id can, so no
/// store is ever taken for it, nor it for a store.
const CURSOR: &str = ".recap.json";
/// The file whose lock a recap holds while it reads and replaces the cursor, and only then.
const LOCK: &str = ".recap.lock";

/// A directory of `BEAD_HOME` that holds a journal: the store it is, or its name as printed and
/// why it is none bead could have made.
pub type Found = Result<Workspace, (String, Error)>;

/// Every directory of `BEAD_HOME` that holds a journal (an `events` directory), in name order. No
/// checkout is consulted and nothing is written; a home that is not there holds none.
pub fn home_stores(settings: &Arc<Settings>) -> Result<Vec<Found>, Error> {
    let mut stores = Vec::new();
    for path in dir_entries(&settings.home()?)? {
        let name = path.file_name().unwrap_or_default();
        let shown = printed_text(name.as_bytes());
        // a temporary file of an atomic write elsewhere in the home may go once listed
        match std::fs::metadata(&path) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => continue,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => {
                stores.push(Err((shown, IoError::at(&error, &path).into())));
                continue;
            }
        }
        let events = path.join("events");
        match std::fs::metadata(&events) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => {
                let why = format!("{} is not a directory", events.display());
                stores.push(Err((shown, Error::io_message(why))));
                continue;
            }
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => {
                stores.push(Err((shown, IoError::at(&error, &events).into())));
                continue;
            }
        }
        // bead names a store by its workspace id, which no other name can be
        let store = (name.to_str())
            .and_then(|id| Workspace::new(Arc::clone(settings), id, None, None, Object::new()).ok())
            .ok_or_else(|| {
                let why = format!(
                    "{} holds a journal, but bead made no store there: its name is not a \
                     workspace id",
                    path.display()
                );
                (shown.clone(), Error::Config(why))
            });
        stores.push(store);
    }
    Ok(stores)
}

/// What a recap has shown of one tracker: the frontier of it (`beadroll_core::recap`), and how
/// many events that covers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mark {
    /// The events nothing else shown builds on, sorted.
    pub frontier: Vec<EventId>,
    /// How many events are below it.
    pub covered: u64,
}

/// What `bead recap` has shown on this machine: each tracker's [`Mark`], by workspace id, and when
/// a recap last marked anything seen. It is this machine's alone, `$BEAD_HOME/.recap.json`, never
/// synced and no part of format 1, and whoever runs bead here shares it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecapCursor {
    /// When a recap last marked activity seen here, in milliseconds since 1970: where a tracker
    /// first seen since starts.
    pub at: Option<u64>,
    /// Each tracker's mark, by workspace id.
    pub marks: BTreeMap<String, Mark>,
}

impl RecapCursor {
    /// The cursor as it is now: an empty one when there is none yet. One that cannot be read, or
    /// is not one, is an error, never taken for an empty one.
    pub fn read(settings: &Settings) -> Result<RecapCursor, Error> {
        read_cursor(&settings.home()?.join(CURSOR))
    }

    /// Records `marks`, each tracker's once a recap's answer has been delivered, and `at`, over the
    /// cursor as it is when this runs: read again under the cursor's lock, where a tracker whose
    /// mark another recap changed since this one read `self` keeps whichever covers more, and the
    /// other trackers keep theirs. The lock is held only to read and replace the file, never while
    /// a journal is read or an answer written.
    pub fn save(
        &self,
        settings: &Settings,
        marks: BTreeMap<String, Mark>,
        at: u64,
    ) -> Result<(), Error> {
        settings.failpoints.hit("recap.before-save");
        let home = settings.home()?;
        private_dir(&home)?;
        let path = home.join(LOCK);
        let lock = OpenOptions::new()
            .read(true)
            .append(true)
            .create(true)
            .mode(0o600)
            .open(&path)
            .map_err(|e| IoError::at(&e, &path))?;
        lock.lock().map_err(|e| IoError::at(&e, &path))?;
        let path = home.join(CURSOR);
        let mut now = read_cursor(&path)?;
        for (id, mark) in marks {
            // another recap marked this tracker since `self` was read: of two frontiers of one
            // growing journal, the one that covers more saw all the other did
            let raced = (now.marks.get(&id)).filter(|theirs| Some(*theirs) != self.marks.get(&id));
            if raced.is_none_or(|theirs| theirs.covered <= mark.covered) {
                now.marks.insert(id, mark);
            }
        }
        now.at = Some(now.at.map_or(at, |was| was.max(at)));
        let text = json::write(&now.to_value(), Format::COMPACT)
            .map_err(|e| Error::Validation(e.to_string()))?;
        // a write that fails says so without the file, as a full volume does: name the cursor
        crate::write_atomic(&path, text.as_bytes(), true, &settings.failpoints).map_err(
            |error| match error {
                Error::Io(mut error) if error.path.is_none() => {
                    error.path = Some(path);
                    Error::Io(error)
                }
                error => error,
            },
        )?;
        drop(lock);
        Ok(())
    }

    /// As its file holds it: `{"version": 1, "at": MILLIS, "workspaces": {ID: {"frontier":
    /// [EVENT, …], "covered": N}}}`, trackers and events sorted.
    fn to_value(&self) -> Value {
        let number = |n: u64| Value::from(i64::try_from(n).unwrap_or(i64::MAX));
        let workspaces: Object = (self.marks.iter())
            .map(|(id, mark)| {
                let frontier = mark.frontier.iter().map(|e| Value::from(e.to_string()));
                let mut entry = Object::new();
                entry.insert("frontier".into(), Value::Array(frontier.collect()));
                entry.insert("covered".into(), number(mark.covered));
                (id.clone(), Value::from(entry))
            })
            .collect();
        let mut value = Object::new();
        value.insert("version".into(), Value::from(1));
        value.insert("at".into(), self.at.map_or(Value::Null, number));
        value.insert("workspaces".into(), Value::from(workspaces));
        Value::from(value)
    }
}

/// The cursor in the file at `path`; an empty one when there is no file.
fn read_cursor(path: &Path) -> Result<RecapCursor, Error> {
    let bad = |why: &str| {
        Error::io_message(format!(
            "{} cannot be used ({why}): repair it, or remove it to start again from the last 24 \
             hours",
            path.display()
        ))
    };
    let value = match read_json_text(path) {
        Ok(value) => value,
        Err(Unreadable::Missing) => return Ok(RecapCursor::default()),
        Err(Unreadable::Other(why)) => return Err(bad(&why)),
    };
    let object = (value.as_object())
        .filter(|object| object.len() == 3 && object.get("version") == Some(&Value::from(1)))
        .ok_or_else(|| bad("not a version 1 cursor"))?;
    let at = match object.get("at") {
        Some(Value::Null) => None,
        at => Some(count(at).ok_or_else(|| bad("at is not a time"))?),
    };
    let workspaces = (object.get("workspaces").and_then(Value::as_object))
        .ok_or_else(|| bad("workspaces is not an object"))?;
    let mut marks = BTreeMap::new();
    for (id, entry) in workspaces {
        let entry = (entry.as_object())
            .filter(|entry| entry.len() == 2)
            .ok_or_else(|| bad("a workspace's mark is not one"))?;
        let frontier = (entry.get("frontier").and_then(Value::as_array))
            .ok_or_else(|| bad("a frontier is not a list"))?
            .iter()
            .map(|event| event.as_str().and_then(EventId::parse))
            .collect::<Option<Vec<EventId>>>()
            .ok_or_else(|| bad("a frontier names something not an event id"))?;
        let covered = count(entry.get("covered")).ok_or_else(|| bad("covered is not a count"))?;
        marks.insert(id.clone(), Mark { frontier, covered });
    }
    Ok(RecapCursor { at, marks })
}

/// A whole number from zero up, as `value` holds it.
fn count(value: Option<&Value>) -> Option<u64> {
    match value {
        Some(Value::Int(int)) => u64::try_from(int.saturating_i128()).ok(),
        _ => None,
    }
}
