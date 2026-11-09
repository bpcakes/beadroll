//! History cache results against an uncached journal, including invalidation and damaged caches.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{File, FileTimes};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use beadroll_core::id::EventId;
use beadroll_core::json::Object;

use super::*;
use crate::{FileMode, Settings, append_events, encode_events, load_journal, write_atomic};

const MACHINE: &str = "0000aa01";

fn workspace(home: &Path, build: &str) -> Workspace {
    let vars = BTreeMap::from([
        (OsString::from("BEAD_HOME"), home.as_os_str().to_owned()),
        (
            OsString::from("BEAD_FAILPOINTS"),
            OsString::from("journal.before-read@always=sleep(0)"),
        ),
    ]);
    let mut settings =
        Settings::from_process(vars, None, OsString::new(), home.to_owned(), &|_| None).unwrap();
    settings.build = Some(build.to_owned());
    Workspace::new(
        Arc::new(settings),
        "history-test",
        None,
        None,
        Object::new(),
    )
    .unwrap()
}

fn id(n: u128) -> String {
    EventId::from_u128((n << 80) | (7 << 76) | (2 << 62) | n).to_string()
}

fn event(n: u128, parent: Option<u128>) -> Value {
    let mut fields = Object::new();
    fields.insert("title".into(), Value::from(format!("value {n}")));
    let mut data = Object::new();
    data.insert("fields".into(), Value::from(fields));
    let mut event = Object::new();
    event.insert("v".into(), Value::from(1));
    event.insert("id".into(), Value::from(id(n)));
    event.insert("replica".into(), Value::from(MACHINE));
    event.insert(
        "deps".into(),
        Value::Array(parent.into_iter().map(|n| Value::from(id(n))).collect()),
    );
    event.insert("op".into(), Value::from("set"));
    event.insert("issue".into(), Value::from(id(1)));
    event.insert("data".into(), Value::from(data));
    Value::from(event)
}

fn oracle(ws: &Workspace) -> Vec<HistoryEntry> {
    let mut journal = load_journal(ws).unwrap();
    let journal = journal.settled_view();
    causal_order(journal)
        .0
        .iter()
        .filter_map(|id| journal.event(*id))
        .map(HistoryEntry::from)
        .collect()
}

fn settle_cache(ws: &Workspace) {
    // A definite later tick without sleeps; the test controls this cache's timestamp.
    File::options()
        .write(true)
        .open(ws.dir.join("history"))
        .unwrap()
        .set_times(FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(4_000_000_000)))
        .unwrap();
}

fn cached_as_journal(ws: &Workspace) {
    let expected = oracle(ws);
    assert_eq!(history(ws).unwrap(), expected);
    settle_cache(ws);
    let before = ws.settings.failpoints.hits("journal.before-read");
    assert_eq!(history(ws).unwrap(), expected);
    assert_eq!(
        ws.settings.failpoints.hits("journal.before-read"),
        before,
        "a hit never parses a segment"
    );
}

#[test]
fn cached_history_invalidates_for_appends_replacements_rotations_and_removal() {
    let home = tempfile::tempdir().unwrap();
    let ws = workspace(home.path(), "one");
    append_events(&ws, MACHINE, [event(1, None), event(2, Some(1))]).unwrap();
    cached_as_journal(&ws);
    append_events(&ws, MACHINE, [event(3, Some(2))]).unwrap();
    cached_as_journal(&ws);
    let segment = segment_files(&ws).unwrap().remove(0);
    write_atomic(
        &segment,
        &encode_events([event(1, None), event(4, Some(1))]).unwrap(),
        FileMode::Private,
        &ws.settings.failpoints,
    )
    .unwrap();
    cached_as_journal(&ws);
    let next = segment.with_file_name("000002.jsonl");
    write_atomic(
        &next,
        &encode_events([event(5, Some(4))]).unwrap(),
        FileMode::Private,
        &ws.settings.failpoints,
    )
    .unwrap();
    cached_as_journal(&ws);
    std::fs::remove_file(next).unwrap();
    cached_as_journal(&ws);
    std::fs::remove_file(segment).unwrap();
    cached_as_journal(&ws);
    assert!(history(&ws).unwrap().is_empty());
}

#[test]
fn a_late_attempt_changes_causal_history_but_a_pending_attempt_does_not() {
    let home = tempfile::tempdir().unwrap();
    let ws = workspace(home.path(), "one");
    append_events(&ws, MACHINE, [event(1, None), event(3, Some(2))]).unwrap();
    cached_as_journal(&ws);
    let data = encode_events([event(2, Some(1))]).unwrap();
    write_atomic(
        &ws.dir.join("pending/attempt.json"),
        &data,
        FileMode::Private,
        &ws.settings.failpoints,
    )
    .unwrap();
    cached_as_journal(&ws);
    let accepted = ws
        .events_dir
        .join(MACHINE)
        .join("claims")
        .join(format!("{}.json", id(2)));
    write_atomic(&accepted, &data, FileMode::Private, &ws.settings.failpoints).unwrap();
    cached_as_journal(&ws);
    assert_eq!(
        history(&ws)
            .unwrap()
            .iter()
            .map(|e| e.id.to_string())
            .collect::<Vec<_>>(),
        [id(1), id(2), id(3)]
    );
    std::fs::remove_file(accepted).unwrap();
    cached_as_journal(&ws);
}

#[test]
fn missing_truncated_corrupt_and_foreign_caches_are_rebuilt() {
    let home = tempfile::tempdir().unwrap();
    let ws = workspace(home.path(), "one");
    append_events(&ws, MACHINE, [event(1, None)]).unwrap();
    cached_as_journal(&ws);
    let path = ws.dir.join("history");
    let bytes = std::fs::read(&path).unwrap();
    for cut in [0, MAGIC.len(), bytes.len() / 2, bytes.len() - 1] {
        std::fs::write(&path, &bytes[..cut]).unwrap();
        cached_as_journal(&ws);
    }
    for at in [0, MAGIC.len(), MAGIC.len() + 16, bytes.len() - 2] {
        let mut broken = bytes.clone();
        broken[at] ^= 1;
        std::fs::write(&path, broken).unwrap();
        cached_as_journal(&ws);
    }
    std::fs::remove_file(&path).unwrap();
    cached_as_journal(&ws);
    let other = workspace(home.path(), "two");
    assert_eq!(history(&other).unwrap(), oracle(&other));
    assert!(other.settings.failpoints.hits("journal.before-read") > 0);
}

#[test]
fn an_unsettled_source_timestamp_cannot_validate_a_cache() {
    let home = tempfile::tempdir().unwrap();
    let ws = workspace(home.path(), "one");
    append_events(&ws, MACHINE, [event(1, None)]).unwrap();
    history(&ws).unwrap();
    let segment = segment_files(&ws).unwrap().remove(0);
    let when = std::fs::metadata(segment).unwrap().modified().unwrap();
    File::options()
        .write(true)
        .open(ws.dir.join("history"))
        .unwrap()
        .set_times(FileTimes::new().set_modified(when))
        .unwrap();
    let before = ws.settings.failpoints.hits("journal.before-read");
    history(&ws).unwrap();
    assert!(ws.settings.failpoints.hits("journal.before-read") > before);
}
