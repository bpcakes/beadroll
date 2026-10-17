//! What bead prints of who held each issue (roll-lqjj.40): two machines that claim, release, take
//! over and close, an offline claim that loses to the owner of the time and one settled later, a
//! close and a release that come too late, and a claim that lost, each write's answer and then the
//! reading commands on both machines, human and `--json`, as this build printed them at 9cbd60a,
//! before an issue's ownership history was kept with its texts; and last, writes that answer with
//! issues whose description, comments and history they read in (a repeated claim, an update of an
//! issue already held, a takeover, updates that close). Ids, keys, machines, commits,
//! times and paths are normalised as the goldens' are (`golden/normal.rs`); a change of output
//! edits `ownership/transcript.txt` by hand in the same change.

use std::path::Path;

use beadroll_core::json::{Format, write};
use beadroll_test_support::events::{Event, id_at};
use beadroll_test_support::{Json, Machine, json};

use crate::golden::Transcript;

/// The issues, imported with keys of their own so that what lists them by key lists them alike on
/// every run.
const ISSUES: &str = r#"{"id":"ow-1","title":"Released","description":"A long text","notes":"kept with the texts","status":"open","priority":1,"issue_type":"task","created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z","created_by":"seed"}
{"id":"ow-2","title":"Taken over, closed too late","description":"Its own text","status":"open","priority":2,"issue_type":"task","created_at":"2026-01-01T01:00:00Z","updated_at":"2026-01-01T01:00:00Z","created_by":"seed","comments":[{"id":21,"issue_id":"ow-2","author":"seed","text":"a comment of its own","created_at":"2026-01-01T01:30:00Z"}]}
{"id":"ow-3","title":"Closed by its owner","status":"open","priority":2,"issue_type":"bug","created_at":"2026-01-01T02:00:00Z","updated_at":"2026-01-01T02:00:00Z","created_by":"seed"}
{"id":"ow-4","title":"Offline claim lost","status":"open","priority":2,"issue_type":"task","created_at":"2026-01-01T03:00:00Z","updated_at":"2026-01-01T03:00:00Z","created_by":"seed"}
{"id":"ow-5","title":"Offline claim settled","status":"open","priority":2,"issue_type":"task","created_at":"2026-01-01T04:00:00Z","updated_at":"2026-01-01T04:00:00Z","created_by":"seed"}
{"id":"ow-6","title":"Taken over, released too late","status":"open","priority":3,"issue_type":"task","created_at":"2026-01-01T05:00:00Z","updated_at":"2026-01-01T05:00:00Z","created_by":"seed"}
{"id":"ow-7","title":"Reopened, claimed, lost to","description":"Text of its own","status":"open","priority":2,"issue_type":"feature","created_at":"2026-01-01T06:00:00Z","updated_at":"2026-01-01T06:00:00Z","created_by":"seed","comments":[{"id":22,"issue_id":"ow-7","author":"seed","text":"another comment","created_at":"2026-01-01T06:30:00Z"}]}
{"id":"ow-8","title":"Closed through update","status":"open","priority":2,"issue_type":"task","created_at":"2026-01-01T07:00:00Z","updated_at":"2026-01-01T07:00:00Z","created_by":"seed"}
"#;

/// The keys of [`ISSUES`].
const KEYS: [&str; 8] = [
    "ow-1", "ow-2", "ow-3", "ow-4", "ow-5", "ow-6", "ow-7", "ow-8",
];

/// What both machines are asked once the work is done (not `info` or `machines`, which list
/// machines by their ids).
const READINGS: &[&[&str]] = &[
    &["export"],
    &["claims"],
    &["claims", "--json"],
    &["prime"],
    &["list", "--all"],
    &["list", "--all", "--json"],
    &["ready"],
    &["ready", "--json"],
    &["blocked"],
    &["blocked", "--json"],
];

fn transcript_path() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cli/commands/ownership/transcript.txt")
}

/// The machine id of `machine`.
fn replica(machine: &Machine) -> String {
    let info = machine.bead(["info"]).expect("info");
    info["machine"].as_str().expect("a machine id").to_owned()
}

/// Two machines' work, recorded on `t`: every ending of a claim there is, and a claim that lost.
fn work(t: &mut Transcript, sandbox: &beadroll_test_support::Sandbox, a: &Machine, b: &Machine) {
    let issues = sandbox.root().join("issues.jsonl");
    std::fs::write(&issues, ISSUES).expect("the export");
    let import = issues.display().to_string();
    t.record(a, &["init", "--prefix", "ow", "--import", &import]);
    t.record(a, &["machines", "--name", &format!("{}=own-a", replica(a))]);
    // texts written here, beside the imported ones (a comment would export a number its id makes)
    t.record(a, &["update", "ow-1", "--design", "drawn here"]);
    t.record(a, &["sync"]);
    t.record(b, &["sync"]);
    // ended by a release, with a reason
    t.record(a, &["claim", "ow-1"]);
    t.record(a, &["release", "ow-1", "-r", "handing back", "--json"]);
    // taken over with a reason, and the old owner's close comes too late: superseded
    t.record(a, &["claim", "ow-2", "--json"]);
    t.record(b, &["sync"]);
    t.record(b, &["takeover", "ow-2", "-r", "a looked dead", "--json"]);
    // a machine has an id once it has written
    t.record(b, &["machines", "--name", &format!("{}=own-b", replica(b))]);
    t.record(a, &["close", "ow-2", "-r", "done"]);
    // ended by its owner's close
    t.record(b, &["claim", "ow-3"]);
    t.record(b, &["close", "ow-3", "-r", "finished", "--json"]);
    // an offline claim while another holds the issue, then that owner's close, which saw it
    t.record(a, &["sync"]);
    t.record(a, &["claim", "ow-4"]);
    t.record(b, &["claim", "ow-4", "--offline", "--json"]);
    t.record(b, &["sync"]);
    t.record(a, &["sync"]);
    t.record(a, &["show", "ow-4"]);
    t.record(a, &["close", "ow-4", "-r", "closed over it"]);
    // an offline claim settled later: claimed online by its owner, then released
    t.record(b, &["claim", "ow-5", "--offline"]);
    t.record(b, &["claim", "ow-5"]);
    t.record(b, &["release", "ow-5"]);
    // taken over while its owner was away, whose release then comes too late
    t.record(a, &["sync"]);
    t.record(a, &["claim", "ow-6"]);
    t.record(b, &["sync"]);
    t.record(b, &["takeover", "ow-6", "-r", "stale"]);
    t.record(a, &["release", "ow-6", "--offline"]);
    t.record(a, &["sync"]);
    // a claim ended by a reopen, and one by a close through update
    t.record(b, &["claim", "ow-7"]);
    t.record(b, &["reopen", "ow-7"]);
    t.record(b, &["claim", "ow-8"]);
    t.record(b, &["update", "ow-8", "--status", "closed", "--json"]);
    t.record(a, &["sync"]);
    t.record(b, &["sync"]);
    t.record(a, &["sync"]);
    t.record(a, &["claim", "ow-7"]);
    plant_lost_claim(a, "ow-7");
    t.note("a third machine's online claim of ow-7, which had not seen a's, planted in a's store");
    t.record(a, &["show", "ow-7"]);
    t.record(a, &["show", "ow-7", "--json"]);
    // an offline claim nobody settles
    t.record(a, &["claim", "ow-1", "--offline"]);
}

/// What a write that reads in the issue's texts to answer prints of issues with a description,
/// comments and a history (ow-7's lost claim, ow-2's close that came too late): a claim and an
/// update of an issue its machine already holds, a takeover, and updates that close, each ending
/// a published claim.
fn again(t: &mut Transcript, a: &Machine) {
    t.note("writes that answer with issues whose texts and histories they read in");
    t.record(a, &["claim", "ow-7", "--json"]);
    t.record(a, &["update", "ow-7", "--claim", "--json"]);
    t.record(a, &["sync"]);
    t.record(a, &["takeover", "ow-2", "-r", "b moved on", "--json"]);
    t.record(a, &["update", "ow-2", "--status", "closed", "--json"]);
    t.record(a, &["update", "ow-7", "--status", "closed", "--json"]);
}

/// Plants in `machine`'s store a third machine's journal holding an online claim of `key` that
/// saw only its create, placed after the claim `machine` holds: no remote accepts two claims of
/// one issue, so a journal holds a claim that lost only when one was planted.
pub(crate) fn plant_lost_claim(machine: &Machine, key: &str) {
    let shown = machine.show(key);
    let held = shown["claim"]["id"].as_str().expect("a claim").to_owned();
    let log = machine.bead(["log", key]).expect("its log");
    let created = (log.as_array().expect("a log").iter())
        .find(|entry| entry["op"].as_str() == Some("create"))
        .and_then(|entry| entry["event"].as_str())
        .expect("its create")
        .to_owned();
    let millis = u64::from_str_radix(&held.replace('-', "")[..12], 16).expect("a time");
    let claim = Event::new(&id_at(millis + 5, 1), "claim")
        .replica("fffffff1")
        .deps(&[&created])
        .issue(&created)
        .data(json!({"mode": "online"}));
    let line = write(&Json::from(claim), Format::COMPACT).expect("a record");
    let journal = machine.store().join("events/fffffff1");
    std::fs::create_dir_all(&journal).expect("a journal's folder");
    std::fs::write(journal.join("000001.jsonl"), format!("{line}\n")).expect("its record");
}

#[test]
fn every_end_of_a_claim_prints_as_it_did() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "own-a");
    let b = crate::rust_only(&sandbox, "own-b");
    let mut t = Transcript::new("ownership", &sandbox);
    work(&mut t, &sandbox, &a, &b);
    for machine in [&a, &b] {
        t.note(&format!("what {} reads", machine.name()));
        for key in KEYS {
            t.record(machine, &["show", key]);
            t.record(machine, &["show", key, "--json"]);
        }
        t.record(machine, &["show", "ow-1", "ow-2", "ow-7", "--json"]);
        for args in READINGS {
            t.record(machine, args);
        }
    }
    again(&mut t, &a);
    t.finish_at(&transcript_path());
}
