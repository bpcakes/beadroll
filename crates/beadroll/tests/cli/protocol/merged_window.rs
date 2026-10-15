//! A journal file written between an ownership round's merge and its transaction (roll-lqjj.37):
//! the claim `bead claim` makes builds on it, as it does on what the merge brought in. The round
//! is held at `ownership.before-transaction`, where nothing writes today, while the test puts
//! another machine's journal file into the store as a change that wrote there would have. The
//! reads the round makes are counted in-process (`beadroll-sync`'s `reads::` tests).

use std::path::Path;

use beadroll_store::{glob_ending, json_text, read_attempt};
use beadroll_test_support::{Json, Machine, Sandbox};

use crate::hardening::gate::{Gate, beside};

/// Machine `a`, which started a tracker of three issues and has written alone, and machine `b`,
/// which joined it.
struct Pair {
    sandbox: Sandbox,
    a: Machine,
    b: Machine,
    keys: [String; 3],
    /// `a`'s folder under `events/`.
    own: String,
}

impl Pair {
    fn new() -> Pair {
        let sandbox = crate::sandbox();
        let (a, b) = (
            crate::rust_only(&sandbox, "a"),
            crate::rust_only(&sandbox, "b"),
        );
        a.bead(["init", "--prefix", "ex"]);
        let issue = |title| {
            let created = a.bead(["create", title]).expect("an issue");
            created["id"].as_str().expect("an id").to_owned()
        };
        let keys = [issue("first"), issue("second"), issue("third")];
        b.bead(["sync"]);
        let mut own = folders(&a);
        assert_eq!(own.len(), 1, "a has written alone: {own:?}");
        let own = own.pop().unwrap_or_default();
        Pair {
            sandbox,
            a,
            b,
            keys,
            own,
        }
    }

    /// `b`'s folder under `events/` once it has written, which `a` has not.
    fn theirs(&self) -> String {
        let mut own: Vec<String> = (folders(&self.b).into_iter())
            .filter(|folder| *folder != self.own)
            .collect();
        assert_eq!(own.len(), 1, "the folders b wrote alone: {own:?}");
        own.pop().unwrap_or_default()
    }

    /// The causes of the claim of the first issue `a` makes while the file at `path` under `b`'s
    /// `events/` is copied to `a`'s, in the window between its merge and its transaction.
    fn causes_of_a_claim_while_copying(&self, path: &str) -> Vec<Json> {
        let window = Gate::new(&self.sandbox, "window");
        let failpoints = format!("ownership.before-transaction={}", window.action());
        let from = self.b.store().join("events").join(path);
        let to = self.a.store().join("events").join(path);
        let (claimed, copied) = beside(
            || {
                let claim = self.a.cmd(["claim", &self.keys[0], "--json"]);
                claim.env("BEAD_FAILPOINTS", &failpoints).run()
            },
            |finished| {
                window.meet(finished, || {
                    let folder = to.parent().expect("a folder");
                    std::fs::create_dir_all(folder).expect("a folder");
                    std::fs::copy(&from, &to).expect("a copy");
                    aged(folder);
                })
            },
        );
        assert!(copied.is_some(), "the claim never reached the window");
        assert_eq!(claimed.returncode(), 0, "{}", claimed.stderr);
        assert!(self.a.show(&self.keys[0]).get("claim").is_some());
        let claims = self.a.store().join("events").join(&self.own).join("claims");
        let attempts = glob_ending(&claims, ".json");
        assert_eq!(attempts.len(), 1, "{attempts:?}");
        let events = read_attempt(&attempts[0], None);
        events[0]["deps"].as_array().expect("causes").to_vec()
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

/// The folders under a machine's `events/`.
fn folders(machine: &Machine) -> Vec<String> {
    let entries = std::fs::read_dir(machine.store().join("events")).expect("a store");
    let names = entries.map(|entry| entry.expect("an entry").file_name());
    names
        .map(|name| name.to_string_lossy().into_owned())
        .collect()
}

/// The id of the last event of the journal file `file`.
fn last_id(file: &Path) -> Json {
    let data = std::fs::read(file).expect("a journal file");
    let last = data.split(|b| *b == b'\n').rfind(|line| !line.is_empty());
    let event = json_text(last.expect("an event")).expect("an event");
    Json::from(event["id"].as_str().expect("an id"))
}

/// Another machine's event is written into the store between the claim's merge and its
/// transaction, as a merge would have brought it in: the claim builds on it, where it built on
/// the heads from before the window when it went on from the merge's projection unchecked.
#[test]
fn a_claim_builds_on_a_segment_written_between_its_merge_and_its_transaction() {
    let s = Pair::new();
    s.b.bead(["comments", "add", &s.keys[1], "meanwhile", "--offline"]);
    let segment = format!("{}/000001.jsonl", s.theirs());
    let written = last_id(&s.b.store().join("events").join(&segment));
    let causes = s.causes_of_a_claim_while_copying(&segment);
    assert!(causes.contains(&written), "{written:?} among {causes:?}");
}

/// The same for an attempt file: another machine's claim of the second issue, accepted by that
/// machine alone.
#[test]
fn a_claim_builds_on_an_attempt_file_written_between_its_merge_and_its_transaction() {
    let s = Pair::new();
    s.b.bead(["claim", &s.keys[1], "--offline"]);
    let folder = s.theirs();
    let attempts = s.b.store().join("events").join(&folder).join("claims");
    let files = glob_ending(&attempts, ".json");
    assert_eq!(files.len(), 1, "{files:?}");
    let name = files[0].file_name().expect("a name").to_string_lossy();
    let written = last_id(&files[0]);
    let causes = s.causes_of_a_claim_while_copying(&format!("{folder}/claims/{name}"));
    assert!(causes.contains(&written), "{written:?} among {causes:?}");
}

/// The same for a claims directory `a` has already, which gains a file: another claim of `b`'s,
/// accepted by `b` alone, written with nothing told to the claim's process.
#[test]
fn a_claim_builds_on_an_attempt_file_added_to_a_claims_directory_it_has() {
    let s = Pair::new();
    s.b.bead(["claim", &s.keys[1]]);
    let folder = s.theirs();
    s.a.bead(["sync"]);
    s.b.bead(["claim", &s.keys[2], "--offline"]);
    let names = |machine: &Machine| -> Vec<std::path::PathBuf> {
        let claims = machine.store().join("events").join(&folder).join("claims");
        glob_ending(&claims, ".json")
    };
    let known = names(&s.a);
    let new: Vec<_> = (names(&s.b).into_iter())
        .filter(|file| !known.iter().any(|k| k.file_name() == file.file_name()))
        .collect();
    assert_eq!(new.len(), 1, "{new:?}");
    let name = new[0].file_name().expect("a name").to_string_lossy();
    let written = last_id(&new[0]);
    let causes = s.causes_of_a_claim_while_copying(&format!("{folder}/claims/{name}"));
    assert!(causes.contains(&written), "{written:?} among {causes:?}");
}
