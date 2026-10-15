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

use crate::fs::content_sum;
use crate::journal::{append_events, load_journal, view, write_attempt_file};
use crate::memo::FileKey;
use crate::settings::Settings;
use crate::workspace::Workspace;

use super::file::{Held, Made};
use super::{file, grown, kept, projection, projection_path, read_texts, resummed, same};

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

/// Moves the projection's own time back behind every file it was made from, so that none of them
/// is settled before it: the file identities then vouch for nothing, as they do not when a command
/// writes the projection in the timestamp tick of the files it has just read.
fn unsettle(ws: &Workspace) {
    let earlier = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
    let projection = std::fs::File::options()
        .write(true)
        .open(projection_path(ws))
        .unwrap();
    projection
        .set_times(std::fs::FileTimes::new().set_modified(earlier))
        .unwrap();
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
    write_attempt_file(&ws, &path, &attempt).unwrap();
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
    // rewritten in place, its length kept, the projection's own time behind it: nothing but the
    // sum kept of the segment can say that its bytes are not the ones it was made from
    unsettle(&ws);
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

/// What a projection was made from and what its files look like now say whether it may be read:
/// an identity settled before the projection was written stands on its own; one of the
/// projection's own timestamp tick, which a rewrite in place at the same length could have kept,
/// has its file read against the sum kept of it; and one with no sum has the journal replayed.
#[test]
fn an_identity_of_the_projections_own_tick_is_read_against_its_sum() {
    let path = format!("{HERE}/000001.jsonl");
    let key = FileKey::from_parts((40, 100, 90, 7, 3));
    let held = |sum| Held {
        path: path.clone(),
        key,
        segment: true,
        sum,
    };
    let made = |sum| Made {
        build: "this build".to_owned(),
        files: vec![held(sum)],
        texts: [0; 16],
    };
    let now = [held(None)];
    let sum = Some([9; 16]);
    assert!(same(&made(sum), &now, &[], 101), "settled: nothing to read");
    assert_eq!(grown(&made(sum), &now, 101).unwrap().len(), 0);
    assert!(!same(&made(sum), &now, &[], 100), "of its own tick");
    let tails = grown(&made(sum), &now, 100).unwrap();
    let read: Vec<(&str, u64, Option<[u8; 16]>)> = (tails.iter())
        .map(|tail| (tail.path.as_str(), tail.from, tail.sum))
        .collect();
    assert_eq!(read, [(path.as_str(), 40, sum)], "read whole, against it");
    assert!(grown(&made(None), &now, 100).is_none(), "nothing summed");
}

/// An attempt file of the projection's own tick is the one it was made from only when its bytes
/// sum to what was kept of them: its identity cannot say, since a rewrite in place that keeps its
/// length keeps every part of it. Whether the identity moved is for the filesystem's clock to
/// decide, so the sum is checked here, where it cannot be skipped.
#[test]
fn an_attempt_of_the_projections_own_tick_is_read_against_its_sum() {
    let home = tempfile::tempdir().unwrap();
    let ws = store(home.path(), Some("this build"));
    let path = ws
        .events_dir
        .join(HERE)
        .join("claims")
        .join(format!("{}.json", id(5)));
    write_attempt_file(&ws, &path, &[title(5, &[4], 2, "before")]).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let now = kept(&ws, &path, FileKey::of(&path), false, None).unwrap();
    let was = |sum| Held { sum, ..now.clone() };
    let summed = was(Some(content_sum(&bytes)));
    // the bytes it was made from: the file stands, and what is kept of it keeps that sum
    assert_eq!(resummed(&ws, &path, &summed, &now), Some(summed.clone()));
    // other bytes than the ones summed, and nothing summed of it at all
    assert_eq!(resummed(&ws, &path, &was(Some([0; 16])), &now), None);
    assert_eq!(resummed(&ws, &path, &was(None), &now), None);
    // another file at that path than the one listed
    let listed = Held {
        key: now.key.up_to(now.key.len() + 1),
        ..now.clone()
    };
    assert_eq!(resummed(&ws, &path, &summed, &listed), None);
    // the bytes changed, its length kept: no sum of them is the one kept
    let changed = String::from_utf8(bytes)
        .unwrap()
        .replace("before", "AFTER!");
    std::fs::write(&path, &changed).unwrap();
    assert_eq!(resummed(&ws, &path, &summed, &now), None);
    let now = kept(&ws, &path, FileKey::of(&path), false, None).unwrap();
    let after = was(Some(content_sum(changed.as_bytes())));
    assert_eq!(
        resummed(&ws, &path, &after, &now),
        Some(Held {
            sum: after.sum,
            ..now
        }),
        "the file it is now, against the sum of its bytes now"
    );
}

/// A cached claims listing says nothing about bytes. Keep the directory unchanged while an
/// accepted attempt changes its title at the same length: projection must answer the new bytes.
#[test]
fn an_attempt_rewritten_in_place_invalidates_the_projection() {
    let home = tempfile::tempdir().unwrap();
    let ws = store(home.path(), Some("this build"));
    begun(&ws);
    let path = ws
        .events_dir
        .join(HERE)
        .join("claims")
        .join(format!("{}.json", id(5)));
    write_attempt_file(&ws, &path, &[title(5, &[4], 2, "before")]).unwrap();
    assert!(replays(&ws));
    unsettle(&ws);
    let data = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, data.replace("before", "AFTER!")).unwrap();
    assert!(replays(&ws));
    let state = view(&ws).unwrap();
    assert!(answers(&state).contains("AFTER!"));
    std::fs::remove_file(&path).unwrap();
    assert!(replays(&ws));
}

#[test]
fn a_new_lock_phase_lists_attempts_afresh() {
    let home = tempfile::tempdir().unwrap();
    let ws = store(home.path(), Some("this build"));
    let path = ws
        .events_dir
        .join(HERE)
        .join("claims")
        .join(format!("{}.json", id(5)));
    write_attempt_file(&ws, &path, &[title(5, &[4], 2, "before")]).unwrap();
    ws.set_holding(true);
    let first = crate::claim_files(&ws).unwrap();
    assert!(std::sync::Arc::ptr_eq(
        &first,
        &crate::claim_files(&ws).unwrap()
    ));
    ws.set_holding(false);
    ws.set_holding(true);
    let again = crate::claim_files(&ws).unwrap();
    assert_eq!(first, again);
    assert!(!std::sync::Arc::ptr_eq(&first, &again));
}

/// An attempt bead writes is listed again whatever the claims directory's times say: one that
/// gains a file within one tick of its listing keeps its identity, so the write forgets the names
/// (Y01).
#[test]
fn an_attempt_written_here_is_listed_whatever_the_times_say() {
    let home = tempfile::tempdir().unwrap();
    let ws = store(home.path(), Some("this build"));
    let claims = ws.events_dir.join(HERE).join("claims");
    let attempt = |n: u128| claims.join(format!("{}.json", id(n)));
    write_attempt_file(&ws, &attempt(5), &[title(5, &[4], 2, "before")]).unwrap();
    ws.set_holding(true);
    let listed = crate::claim_files(&ws).unwrap();
    assert_eq!(&*listed, &[attempt(5)]);
    assert_eq!(ws.claims.borrow().listed(), 1, "the directory's names");
    let when = std::fs::metadata(&claims).unwrap().modified().unwrap();
    write_attempt_file(&ws, &attempt(6), &[title(6, &[5], 2, "after")]).unwrap();
    assert_eq!(ws.claims.borrow().listed(), 0, "forgotten by the write");
    // the directory's time put back where it was when it was listed
    let opened = std::fs::File::open(&claims).unwrap();
    opened
        .set_times(std::fs::FileTimes::new().set_modified(when))
        .unwrap();
    let both = crate::claim_files(&ws).unwrap();
    assert_eq!(&*both, &[attempt(5), attempt(6)]);
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

mod attempts;
mod texts;
