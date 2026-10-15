//! How often a sync and a write read the state of the store's journal (roll-lqjj.29): once per
//! round of a sync, by its merge, which the report and an ownership command's transaction go on
//! from, and once for a write's transaction. Counted at the `projection.read` failpoint. An
//! ownership command's transaction goes on from the merge's read only while the journal's files
//! are still the ones it was made from: a file written between the two has the store read again
//! (roll-lqjj.37).

use std::path::{Path, PathBuf};

use beadroll_core::json::{Format, Object, Value, write};
use beadroll_store::{Endpoint, Workspace, glob_ending, json_text, read_attempt, replica_id};
use beadroll_sync::{publish_soon, synchronise, writing};
use beadroll_test_support::{Machine, Sandbox, json};

use crate::support::{Gate, actor, alongside, claim_command, sandbox, text, workspace};

/// A spec that never fires and counts every read.
const COUNTED: (&str, &str) = ("BEAD_FAILPOINTS", "projection.read@nth(1000000)=sleep(0)");

/// How often `ws` has read its store's state.
fn reads(ws: &Workspace) -> u64 {
    ws.settings.failpoints.hits("projection.read")
}

/// A sync that brings nothing in reads the state once, and reports it as before.
#[test]
fn a_sync_that_brings_nothing_reads_the_state_once() {
    let sandbox = sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    a.bead(["create", "x"]);
    a.bead(["create", "y", "--offline"]);
    let ws = workspace(&a, sandbox.origin(), &[COUNTED]);
    let synced = synchronise(&ws, &Endpoint::at(sandbox.origin())).expect("a sync");
    assert_eq!(reads(&ws), 1);
    assert_eq!((synced.pulled, synced.pushed, synced.issues), (0, 1, 2));
    assert!(synced.renamed.is_empty(), "{:?}", synced.renamed);
    let again = synchronise(&ws, &Endpoint::at(sandbox.origin())).expect("a sync");
    assert_eq!(reads(&ws), 2);
    assert_eq!((again.pulled, again.pushed, again.issues), (0, 0, 2));
}

/// A sync that brings records in reads the state before it merges, to count the keys renamed
/// before it, and after: the copy the merge renamed is reported, and only it.
#[test]
fn a_sync_that_brings_a_rename_reads_the_state_before_and_after_its_merge() {
    let sandbox = sandbox();
    let record = json!({
        "id": "ex-a",
        "title": "A",
        "status": "open",
        "priority": 2,
        "issue_type": "task",
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-01T00:00:00Z",
        "created_by": "seed",
    });
    let path = sandbox.root().join("ext.jsonl");
    let line = write(&record, Format::ASCII).expect("a record") + "\n";
    std::fs::write(&path, line).unwrap();
    let path = path.to_str().expect("a UTF-8 path");
    let (m1, m2) = (Machine::new(&sandbox, "m1"), Machine::new(&sandbox, "m2"));
    m1.bead(["init", "--prefix", "ex"]);
    m2.bead(["sync"]);
    m1.bead(["import", path]);
    m2.bead(["import", path, "--offline"]);
    let ws = workspace(&m2, sandbox.origin(), &[COUNTED]);
    let synced = synchronise(&ws, &Endpoint::at(sandbox.origin())).expect("a sync");
    assert_eq!(reads(&ws), 2);
    assert_eq!((synced.pulled, synced.issues), (1, 2));
    assert_eq!(synced.renamed.len(), 1, "{:?}", synced.renamed);
    let renamed = &synced.renamed[0];
    assert_eq!(renamed.get("from").and_then(Value::as_str), Some("ex-a"));
    // the next sync brings nothing: nothing more is renamed since it began
    let again = synchronise(&ws, &Endpoint::at(sandbox.origin())).expect("a sync");
    assert!(again.renamed.is_empty(), "{:?}", again.renamed);
    assert_eq!(reads(&ws), 3);
}

/// A write published at once reads the state for its transaction, then once for the merge of the
/// sync that publishes it, which makes no report.
#[test]
fn a_published_write_reads_the_state_for_its_transaction_and_its_merge() {
    let sandbox = sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let key = text(&a.bead(["create", "x"]).expect("an issue")["id"]);
    let ws = workspace(&a, sandbox.origin(), &[COUNTED]);
    let actor = actor(&a);
    writing(&ws, &actor, |tx| {
        let uid = tx.state()?.resolve(&key).expect("the issue").uid();
        let mut data = Object::new();
        data.insert("text".into(), Value::from("noted"));
        tx.emit("comment", Some(&uid.to_string()), data, None)
    })
    .expect("a comment");
    assert_eq!(reads(&ws), 1);
    assert_eq!(publish_soon(&ws, false, None), None);
    assert_eq!(reads(&ws), 2);
    let shown = a.show(&key);
    assert_eq!(shown["comments"][0]["text"].as_str(), Some("noted"));
}

/// An ownership command's round reads the state once, by its merge, which its transaction starts
/// from.
#[test]
fn a_claim_reads_the_state_once_a_round() {
    let sandbox = sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let key = text(&a.bead(["create", "x"]).expect("an issue")["id"]);
    let ws = workspace(&a, sandbox.origin(), &[COUNTED]);
    claim_command(&ws, &actor(&a), &key).expect("a claim");
    assert_eq!(reads(&ws), 1);
    assert!(a.show(&key).get("claim").is_some());
}

/// Two machines on one tracker: `a` has published three issues, `b` has joined and is about to write
/// what `a` has not seen.
struct Window {
    sandbox: Sandbox,
    a: Machine,
    b: Machine,
    keys: [String; 3],
}

impl Window {
    fn new() -> Window {
        let sandbox = sandbox();
        let (a, b) = (Machine::new(&sandbox, "a"), Machine::new(&sandbox, "b"));
        a.bead(["init", "--prefix", "ex"]);
        let key = || text(&a.bead(["create", "x"]).expect("an issue")["id"]);
        let keys = [key(), key(), key()];
        b.bead(["sync"]);
        Window {
            sandbox,
            a,
            b,
            keys,
        }
    }

    /// What `b` wrote on its own, kept to itself (`--offline`): its folder under `events/`, which
    /// `a` has not.
    fn theirs(&self) -> String {
        let folders = |machine: &Machine| -> Vec<String> {
            let events = machine.store().join("events");
            let names = std::fs::read_dir(events).expect("a store's events");
            names
                .map(|entry| entry.expect("an entry").file_name())
                .map(|name| name.to_string_lossy().into_owned())
                .collect()
        };
        let known = folders(&self.a);
        let own: Vec<String> = (folders(&self.b).into_iter())
            .filter(|folder| !known.contains(folder))
            .collect();
        assert_eq!(own.len(), 1, "the folders b wrote alone: {own:?}");
        own.into_iter().next().unwrap_or_default()
    }

    /// `path` under `b`'s `events/`, copied to where `a` would have it had its merge brought it in:
    /// the write of the window. Written from outside `a`'s process, which does not forget the names
    /// it listed (`Workspace::forget_claims`).
    fn copy_over(&self, path: &str) -> impl FnOnce() + Send + use<> {
        let from = self.b.store().join("events").join(path);
        let to = self.a.store().join("events").join(path);
        move || {
            let folder = to.parent().expect("a folder");
            std::fs::create_dir_all(folder).expect("a folder");
            std::fs::copy(from, &to).expect("a copy");
            aged(folder);
        }
    }

    /// What `b`'s copy of the segment at `path` under `events/` has beyond `a`'s, appended to
    /// `a`'s, as a merge extends a segment: the write of the window, from outside `a`'s process.
    fn append_rest(&self, path: &str) -> impl FnOnce() + Send + use<> {
        let from = self.b.store().join("events").join(path);
        let to = self.a.store().join("events").join(path);
        move || {
            let (theirs, ours) = (std::fs::read(from).unwrap(), std::fs::read(&to).unwrap());
            let rest = theirs.strip_prefix(ours.as_slice()).expect("a longer copy");
            let mut segment = std::fs::OpenOptions::new().append(true).open(to).unwrap();
            std::io::Write::write_all(&mut segment, rest).expect("an append");
        }
    }

    /// The file at `path` under `a`'s `events/` replaced by another of the same bytes renamed over
    /// it, as an atomic write does, the names it leaves the same: the write of the window, from
    /// outside `a`'s process too.
    fn rename_over(&self, path: &str) -> impl FnOnce() + Send + use<> {
        let target = self.a.store().join("events").join(path);
        move || {
            let new = target.with_extension("json.window");
            std::fs::copy(&target, &new).expect("a copy");
            std::fs::rename(&new, &target).expect("a rename");
            aged(target.parent().expect("a folder"));
        }
    }

    /// The attempt file of `folder` under `events/` that `b` has and `a` has not.
    fn lacking(&self, folder: &str) -> String {
        let names = |machine: &Machine| -> Vec<String> {
            let claims = machine.store().join("events").join(folder).join("claims");
            let files = glob_ending(&claims, ".json");
            let name = |file: &PathBuf| file.file_name().map(|n| n.to_string_lossy().into_owned());
            files.iter().filter_map(name).collect()
        };
        let known = names(&self.a);
        let own: Vec<String> = (names(&self.b).into_iter())
            .filter(|name| !known.contains(name))
            .collect();
        assert_eq!(own.len(), 1, "the attempt files b wrote alone: {own:?}");
        own.into_iter().next().unwrap_or_default()
    }

    /// `a` claims the first issue, held at the window between its merge and its transaction,
    /// where `write` is done: the claim, `a`'s store and how often its state was read.
    fn claim_while(&self, write: impl FnOnce() + Send) -> (Workspace, u64) {
        let gate = Gate::new(&self.sandbox, "window");
        let spec = format!(
            "projection.read@nth(1000000)=sleep(0);ownership.before-transaction={}",
            gate.action()
        );
        let ws = workspace(
            &self.a,
            self.sandbox.origin(),
            &[("BEAD_FAILPOINTS", &spec)],
        );
        let actor = actor(&self.a);
        let (claimed, met) = alongside(
            || claim_command(&ws, &actor, &self.keys[0]),
            |done| gate.meet(done, write),
        );
        claimed.expect("a claim");
        assert!(met.is_some(), "the claim never reached the window");
        let reads = reads(&ws);
        (ws, reads)
    }

    /// The causes of the claim `a` made: the events its event builds on.
    fn causes(&self, ws: &Workspace) -> Vec<String> {
        let replica = replica_id(&ws.settings, false).expect("an identity");
        let claims = ws
            .events_dir
            .join(replica.expect("a replica"))
            .join("claims");
        let files = glob_ending(&claims, ".json");
        assert_eq!(files.len(), 1, "the claims accepted: {files:?}");
        let events = read_attempt(&files[0], None);
        assert_eq!(events.len(), 1, "{events:?}");
        let deps = events[0]["deps"].as_array().expect("causes");
        deps.iter().map(text).collect()
    }
}

/// Puts the modification time of the directory `dir` back a minute, so that a change made in it is
/// seen however coarse the clock its times come from is: a directory that gains a file within one
/// tick of its listing keeps its identity (X13).
fn aged(dir: &Path) {
    let earlier = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
    let opened = std::fs::File::open(dir).expect("a directory");
    let times = std::fs::FileTimes::new().set_modified(earlier);
    opened.set_times(times).expect("a time");
}

/// The id of the last event of `file`, a segment, or of the attempt file `file`.
fn last_id(file: &Path) -> String {
    let data = std::fs::read(file).expect("a journal file");
    let last = data.split(|b| *b == b'\n').rfind(|line| !line.is_empty());
    let event = json_text(last.expect("an event")).expect("an event");
    text(&event["id"])
}

/// A segment another machine's merge would have brought in is written between a claim's merge and
/// its transaction. The transaction starts from the store as it is, which holds that machine's
/// event, and the claim builds on it; before the guard it started from the merge's projection and
/// built on the heads from before.
#[test]
fn a_segment_written_after_the_merge_is_read_by_the_claims_transaction() {
    let s = Window::new();
    s.b.bead([
        "comments",
        "add",
        &s.keys[1],
        "written meanwhile",
        "--offline",
    ]);
    let segment = format!("{}/000001.jsonl", s.theirs());
    let written = last_id(&s.b.store().join("events").join(&segment));
    let (ws, reads) = s.claim_while(s.copy_over(&segment));
    assert_eq!(reads, 2, "the merge's read, and the store read again");
    let causes = s.causes(&ws);
    assert!(causes.contains(&written), "{written} among {causes:?}");
    assert!(s.a.show(&s.keys[0]).get("claim").is_some());
}

/// The likeliest write of all: an event appended to a segment `a` has already, as a merge extends
/// one. The segment's length is all that says so, and the claim builds on the event appended.
#[test]
fn an_event_appended_to_a_segment_after_the_merge_is_read_by_the_claims_transaction() {
    let s = Window::new();
    s.b.bead(["comments", "add", &s.keys[1], "published"]);
    let theirs = s.theirs();
    s.a.bead(["sync"]);
    s.b.bead(["comments", "add", &s.keys[1], "kept back", "--offline"]);
    let segment = format!("{theirs}/000001.jsonl");
    let written = last_id(&s.b.store().join("events").join(&segment));
    let (ws, reads) = s.claim_while(s.append_rest(&segment));
    assert_eq!(reads, 2, "the merge's read, and the store read again");
    let causes = s.causes(&ws);
    assert!(causes.contains(&written), "{written} among {causes:?}");
}

/// The same for an attempt file: the claim of another issue, accepted by `b` alone, is written
/// into `a`'s store between its claim's merge and its transaction.
#[test]
fn an_attempt_file_written_after_the_merge_is_read_by_the_claims_transaction() {
    let s = Window::new();
    s.b.bead(["claim", &s.keys[1], "--offline"]);
    let theirs = s.theirs();
    let attempts = s.b.store().join("events").join(&theirs).join("claims");
    let files = glob_ending(&attempts, ".json");
    assert_eq!(files.len(), 1, "{files:?}");
    let name = files[0].file_name().expect("a name").to_string_lossy();
    let written = last_id(&files[0]);
    let (ws, reads) = s.claim_while(s.copy_over(&format!("{theirs}/claims/{name}")));
    assert_eq!(reads, 2, "the merge's read, and the store read again");
    let causes = s.causes(&ws);
    assert!(causes.contains(&written), "{written} among {causes:?}");
}

/// The same for a claims directory `a` has already, which gains a file: another claim of `b`'s,
/// accepted by `b` alone, written from outside the process with the phase not told.
#[test]
fn an_attempt_file_added_to_a_claims_directory_after_the_merge_is_read_by_the_claims_transaction() {
    let s = Window::new();
    s.b.bead(["claim", &s.keys[1]]);
    let theirs = s.theirs();
    s.a.bead(["sync"]);
    s.b.bead(["claim", &s.keys[2], "--offline"]);
    let name = s.lacking(&theirs);
    let written = last_id(
        &s.b.store()
            .join("events")
            .join(&theirs)
            .join("claims")
            .join(&name),
    );
    let (ws, reads) = s.claim_while(s.copy_over(&format!("{theirs}/claims/{name}")));
    assert_eq!(reads, 2, "the merge's read, and the store read again");
    let causes = s.causes(&ws);
    assert!(causes.contains(&written), "{written} among {causes:?}");
}

/// And for an attempt file `a` has, replaced in place by a rename: the names are the same, the
/// claims directory is not.
#[test]
fn an_attempt_file_replaced_after_the_merge_is_read_by_the_claims_transaction() {
    let s = Window::new();
    s.b.bead(["claim", &s.keys[1]]);
    let theirs = s.theirs();
    s.a.bead(["sync"]);
    let claims = s.a.store().join("events").join(&theirs).join("claims");
    let files = glob_ending(&claims, ".json");
    assert_eq!(files.len(), 1, "{files:?}");
    let name = files[0].file_name().expect("a name").to_string_lossy();
    let (_, reads) = s.claim_while(s.rename_over(&format!("{theirs}/claims/{name}")));
    assert_eq!(reads, 2, "the merge's read, and the store read again");
}

/// The guard does not read the store again for what the merge itself wrote: a claim whose merge
/// brought in a segment, or an attempt file, goes on from the projection it read after them, and
/// reads the state once, as a claim whose merge brought nothing does.
#[test]
fn a_claim_whose_merge_wrote_files_still_reads_the_state_once() {
    let writes: [&[&str]; 2] = [&["comments", "add", "KEY", "meanwhile"], &["claim", "KEY"]];
    for write in writes {
        let s = Window::new();
        let args = write.iter().map(|arg| arg.replace("KEY", &s.keys[1]));
        s.b.bead(args.collect::<Vec<String>>());
        let ws = workspace(&s.a, s.sandbox.origin(), &[COUNTED]);
        claim_command(&ws, &actor(&s.a), &s.keys[0]).expect("a claim");
        assert_eq!(reads(&ws), 1, "after {write:?}");
        let causes = s.causes(&ws);
        assert_eq!(causes.len(), 1, "{causes:?}");
        assert!(s.a.show(&s.keys[0]).get("claim").is_some());
    }
}
