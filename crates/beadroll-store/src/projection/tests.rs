//! A store's projection answers as its journal replayed, whatever happened to either: records
//! appended, files replaced or rewritten, a projection damaged, cut, removed or written by another
//! build. Every answer is checked against replay.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use beadroll_core::id::EventId;
use beadroll_core::json::{Object, Value};
use beadroll_core::replay::replay;
use beadroll_core::state::{Issue, State};

use crate::journal::{append_events, load_journal, view, write_attempt_file};
use crate::settings::Settings;
use crate::workspace::Workspace;

use super::{file, projection, projection_path, read_texts};

const HERE: &str = "0000aa01";

/// A store under `home`, its projection kept for `build` (`None`: none kept).
fn store(home: &Path, build: Option<&str>) -> Workspace {
    failing(home, build, None)
}

/// [`store`], with the failpoints `failpoints`.
fn failing(home: &Path, build: Option<&str>, failpoints: Option<&str>) -> Workspace {
    let mut vars: BTreeMap<OsString, OsString> = BTreeMap::new();
    vars.insert("BEAD_HOME".into(), home.as_os_str().to_owned());
    if let Some(failpoints) = failpoints {
        vars.insert("BEAD_FAILPOINTS".into(), failpoints.into());
    }
    let mut settings =
        Settings::from_process(vars, None, OsString::new(), std::env::temp_dir(), &|_| None)
            .unwrap();
    settings.build = build.map(str::to_owned);
    let ws = Workspace::new(Arc::new(settings), "store", None, None, Object::new()).unwrap();
    std::fs::create_dir_all(ws.events_dir.join(HERE)).unwrap();
    ws
}

fn id(n: u128) -> String {
    let value = ((1_790_000_000_000 + n) << 80) | (7 << 76) | (2 << 62) | n;
    EventId::from_u128(value).to_string()
}

fn event(n: u128, deps: &[u128], op: &str, issue: u128, data: Object) -> Value {
    let mut event = Object::new();
    event.insert("v".into(), Value::from(1));
    event.insert("id".into(), Value::from(id(n)));
    event.insert("replica".into(), Value::from(HERE));
    let deps = deps.iter().map(|dep| Value::from(id(*dep))).collect();
    event.insert("deps".into(), Value::Array(deps));
    event.insert("op".into(), Value::from(op));
    event.insert("issue".into(), Value::from(id(issue)));
    event.insert("data".into(), Value::from(data));
    Value::from(event)
}

fn create(n: u128, deps: &[u128], key: &str) -> Value {
    let mut fields = Object::new();
    fields.insert("title".into(), Value::from(key));
    let mut data = Object::new();
    data.insert("key".into(), Value::from(key));
    data.insert("fields".into(), Value::from(fields));
    event(n, deps, "create", n, data)
}

fn title(n: u128, deps: &[u128], issue: u128, title: &str) -> Value {
    let mut fields = Object::new();
    fields.insert("title".into(), Value::from(title));
    let mut data = Object::new();
    data.insert("fields".into(), Value::from(fields));
    event(n, deps, "set", issue, data)
}

/// A tracker of two issues, the second retitled: the frontier is event 4.
fn begun(ws: &Workspace) {
    let mut init = Object::new();
    init.insert("prefix".into(), Value::from("pr"));
    let events = [
        event(1, &[], "init", 1, init),
        create(2, &[1], "pr-a1"),
        create(3, &[2], "pr-b2"),
        title(4, &[3], 3, "renamed"),
    ];
    append_events(ws, HERE, &events).unwrap();
}

/// What a state answers: its issues by key, its integrity reports, its renames.
fn answers(state: &State) -> String {
    let issues: Vec<String> = (state.issues_by_key().iter())
        .map(|issue| format!("{issue:?}"))
        .collect();
    format!("{issues:?} {:?} {:?}", state.integrity(), state.renames())
}

/// Whether `ws` answers as its journal replayed, through its projection, its texts read in, and
/// through `view`.
fn replays(ws: &Workspace) -> bool {
    let replayed = answers(&replay(load_journal(ws).unwrap()).unwrap());
    let mut projected = projection(ws).unwrap().into_state();
    let uids: Vec<EventId> = projected.issues().map(Issue::uid).collect();
    read_texts(ws, &mut projected, uids)
        && answers(&view(ws).unwrap()) == replayed
        && answers(&projected) == replayed
}

/// What the stored projection was made from, by path: each file's length.
fn made(ws: &Workspace) -> Option<BTreeMap<String, u64>> {
    let stored = file::read(&projection_path(ws))?;
    Some(
        (stored.made.files.iter())
            .map(|held| (held.path.clone(), held.key.len()))
            .collect(),
    )
}

fn segment(ws: &Workspace) -> PathBuf {
    ws.events_dir.join(HERE).join("000001.jsonl")
}

#[test]
fn a_projection_is_kept_and_read_back_as_the_journal_replayed() {
    let home = tempfile::tempdir().unwrap();
    let ws = store(home.path(), Some("this build"));
    begun(&ws);
    assert!(made(&ws).is_none(), "nothing is kept before a command asks");
    assert!(replays(&ws));
    let size = std::fs::metadata(segment(&ws)).unwrap().len();
    let expected = BTreeMap::from([(format!("{HERE}/000001.jsonl"), size)]);
    assert_eq!(made(&ws), Some(expected));
    // read back: the same answers
    assert!(replays(&ws));
}

#[test]
fn records_appended_are_applied_after_it() {
    let home = tempfile::tempdir().unwrap();
    let ws = store(home.path(), Some("this build"));
    begun(&ws);
    assert!(replays(&ws));
    append_events(&ws, HERE, &[title(5, &[4], 2, "later")]).unwrap();
    let mut claim = Object::new();
    claim.insert("mode".into(), Value::from("offline"));
    let attempt = [event(6, &[5], "claim", 2, claim)];
    let path = ws
        .events_dir
        .join(HERE)
        .join("claims")
        .join(format!("{}.json", id(6)));
    write_attempt_file(&path, &attempt, &ws.settings.failpoints).unwrap();
    assert!(replays(&ws));
    let made = made(&ws).unwrap();
    assert_eq!(made.len(), 2, "{made:?}");
    let size = std::fs::metadata(segment(&ws)).unwrap().len();
    assert_eq!(made.get(&format!("{HERE}/000001.jsonl")), Some(&size));
}

#[test]
fn a_journal_changed_otherwise_is_replayed() {
    let home = tempfile::tempdir().unwrap();
    let ws = store(home.path(), Some("this build"));
    begun(&ws);
    assert!(replays(&ws));
    // rewritten in place, its length kept: the second title is another
    let data = std::fs::read_to_string(segment(&ws)).unwrap();
    std::fs::write(segment(&ws), data.replace("renamed", "RENAMED")).unwrap();
    assert!(replays(&ws));
    // replaced by a shorter one
    let lines: Vec<&str> = data.lines().take(3).collect();
    std::fs::write(segment(&ws), format!("{}\n", lines.join("\n"))).unwrap();
    assert!(replays(&ws));
    // a line not yet whole: replayed, and nothing kept of it
    let before = made(&ws);
    let mut torn = std::fs::read(segment(&ws)).unwrap();
    torn.extend_from_slice(b"{\"v\": 1, \"id\"");
    std::fs::write(segment(&ws), torn).unwrap();
    assert!(replays(&ws));
    assert_eq!(made(&ws), before);
}

#[test]
fn a_damaged_projection_is_never_trusted() {
    let home = tempfile::tempdir().unwrap();
    let ws = store(home.path(), Some("this build"));
    begun(&ws);
    assert!(replays(&ws));
    let path = projection_path(&ws);
    let good = std::fs::read(&path).unwrap();
    // every cut, and a byte changed anywhere, is no projection: replayed, and written again whole
    let mut damaged: Vec<Vec<u8>> = (0..good.len()).map(|cut| good[..cut].to_vec()).collect();
    for at in (0..good.len()).step_by(7) {
        let mut flipped = good.clone();
        flipped[at] ^= 0x10;
        damaged.push(flipped);
    }
    for bytes in damaged {
        std::fs::write(&path, &bytes).unwrap();
        assert!(file::read(&path).and_then(file::Stored::encoded).is_none());
        assert!(replays(&ws));
        assert_eq!(std::fs::read(&path).unwrap(), good);
    }
    std::fs::remove_file(&path).unwrap();
    assert!(replays(&ws));
    assert_eq!(std::fs::read(&path).unwrap(), good);
}

#[test]
fn another_builds_projection_is_not_read() {
    let home = tempfile::tempdir().unwrap();
    let ws = store(home.path(), Some("one build"));
    begun(&ws);
    assert!(replays(&ws));
    // another build replays, and keeps its own projection
    let other = store(home.path(), Some("another build"));
    assert!(replays(&other));
    let stored = file::read(&projection_path(&ws)).unwrap();
    assert_eq!(stored.made.build, "another build");
    // a bead that keeps none reads none, and writes none
    std::fs::remove_file(projection_path(&ws)).unwrap();
    let none = store(home.path(), None);
    assert!(replays(&none));
    assert!(!projection_path(&ws).exists());
}

#[test]
fn nothing_is_kept_where_there_is_no_store() {
    let home = tempfile::tempdir().unwrap();
    let ws = store(home.path(), Some("this build"));
    std::fs::remove_dir_all(&ws.dir).unwrap();
    assert!(view(&ws).unwrap().prefix().is_none());
    assert!(!ws.dir.exists(), "a read made the store");
}

#[test]
fn a_projection_that_cannot_be_written_leaves_the_last_one() {
    let home = tempfile::tempdir().unwrap();
    let ws = store(home.path(), Some("this build"));
    begun(&ws);
    assert!(replays(&ws));
    let before = std::fs::read(projection_path(&ws)).unwrap();
    append_events(&ws, HERE, &[title(5, &[4], 2, "later")]).unwrap();
    // the volume is full as the next one is written
    let full = failing(
        home.path(),
        Some("this build"),
        Some("atomic-write:/projection@always=errno(ENOSPC)"),
    );
    assert!(replays(&full));
    assert_eq!(std::fs::read(projection_path(&ws)).unwrap(), before);
    let left: Vec<_> = std::fs::read_dir(&ws.dir)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert!(
        !left
            .iter()
            .any(|name| name.to_string_lossy().ends_with(".tmp")),
        "{left:?}"
    );
    // the next command takes the appended record in, and keeps it
    assert!(replays(&ws));
    assert_ne!(std::fs::read(projection_path(&ws)).unwrap(), before);
}

#[test]
fn a_segment_rewritten_and_then_appended_to_is_replayed() {
    let home = tempfile::tempdir().unwrap();
    let ws = store(home.path(), Some("this build"));
    begun(&ws);
    assert!(replays(&ws));
    // the second title rewritten where it is, its length kept, then a record appended: the same
    // file, longer, but not what the projection was made from followed by more
    let data = std::fs::read_to_string(segment(&ws)).unwrap();
    std::fs::write(segment(&ws), data.replace("renamed", "RENAMED")).unwrap();
    append_events(&ws, HERE, &[title(5, &[4], 2, "later")]).unwrap();
    assert!(replays(&ws));
}

#[test]
fn an_earlier_segment_that_grows_is_replayed() {
    let home = tempfile::tempdir().unwrap();
    let ws = store(home.path(), Some("this build"));
    let line = |value: &Value| {
        let mut line = beadroll_core::json::write(value, beadroll_core::json::Format::COMPACT)
            .unwrap()
            .into_bytes();
        line.push(b'\n');
        line
    };
    let mut init = Object::new();
    init.insert("prefix".into(), Value::from("pr"));
    let first = [event(1, &[], "init", 1, init), create(2, &[1], "pr-a1")];
    std::fs::write(
        segment(&ws),
        first.iter().flat_map(line).collect::<Vec<u8>>(),
    )
    .unwrap();
    let second = ws.events_dir.join(HERE).join("000002.jsonl");
    std::fs::write(&second, line(&title(3, &[2], 2, "second"))).unwrap();
    assert!(replays(&ws));
    // a record put in the first segment, behind the second's: in the journal it comes before it,
    // and the second no longer builds on its predecessor
    let mut labels = Object::new();
    labels.insert("labels".into(), Value::Array(vec![Value::from("ops")]));
    let late = line(&event(4, &[3], "label_add", 2, labels));
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(segment(&ws))
        .unwrap();
    std::io::Write::write_all(&mut file, &late).unwrap();
    drop(file);
    assert!(replays(&ws));
}

mod texts;
