//! The store's projection (roll-lqjj.2), from outside: bead answers alike with it, without it and
//! with a damaged one, across machines that sync, and a bead killed as it writes one leaves the
//! last one in place. Every machine here runs the `bead` this package builds (`rust_only`): the
//! projection is this build's.

use std::path::PathBuf;

use beadroll_test_support::Machine;

/// The reading commands whose answers are compared.
const READS: [&[&str]; 5] = [
    &["list", "--json"],
    &["list", "-a", "--json"],
    &["ready", "--json"],
    &["blocked", "--json"],
    &["export"],
];

fn projection(machine: &Machine) -> PathBuf {
    machine.store().join("projection")
}

/// What `machine` answers to the reading commands and to `show` of each of `keys`; with
/// `replaying`, its projection is removed before each, so that each replays the journal.
fn answers(machine: &Machine, keys: &[String], replaying: bool) -> Vec<String> {
    let mut out = Vec::new();
    let shows: Vec<Vec<&str>> = keys.iter().map(|key| vec!["show", key, "--json"]).collect();
    for args in READS.iter().map(|args| args.to_vec()).chain(shows) {
        if replaying {
            let _ = std::fs::remove_file(projection(machine));
        }
        let run = machine.run(&args);
        assert_eq!(run.returncode(), 0, "{args:?}: {}", run.stderr);
        out.push(run.stdout);
    }
    out
}

/// A tracker with issues that depend on each other, labels, comments, a claim and a close; the
/// keys of its issues.
fn tracker(machine: &Machine) -> Vec<String> {
    machine.bead(["init", "--prefix", "ex"]);
    let mut keys = Vec::new();
    for title in ["first", "second", "third"] {
        let created = machine.bead(["create", title]).expect("an issue");
        keys.push(created["id"].as_str().expect("its key").to_owned());
    }
    let (first, second, third) = (&keys[0], &keys[1], &keys[2]);
    machine.bead(["dep", "add", second, first]);
    machine.bead([
        "update",
        third,
        "--add-label",
        "ops",
        "--title",
        "third, again",
    ]);
    machine.bead(["comments", "add", first, "a note"]);
    machine.bead(["claim", first]);
    machine.bead(["close", second, "-r", "done"]);
    keys
}

#[test]
fn bead_answers_alike_with_and_without_its_projection() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "kept-a");
    let keys = tracker(&a);
    let kept = answers(&a, &keys, false);
    assert!(projection(&a).is_file(), "a read keeps a projection");
    assert_eq!(answers(&a, &keys, true), kept);
    // a damaged projection is no projection: a read replays, and writes it again
    std::fs::write(projection(&a), b"bead projection\ndamaged").unwrap();
    assert_eq!(answers(&a, &keys, false), kept);
    let bytes = std::fs::read(projection(&a)).unwrap();
    let made = u64::from_le_bytes(bytes[24..32].try_into().unwrap()) as usize;
    let mut flipped = bytes.clone();
    flipped[96 + made + 5] ^= 0x01; // in the state
    std::fs::write(projection(&a), &flipped).unwrap();
    assert_eq!(answers(&a, &keys, false), kept);
    assert_eq!(std::fs::read(projection(&a)).unwrap(), bytes);
    // damage where only a write reads: the reads answer from the state, the write replays
    let mut flipped = bytes;
    let last = flipped.len() - 1;
    flipped[last] ^= 0x01;
    std::fs::write(projection(&a), &flipped).unwrap();
    assert_eq!(answers(&a, &keys, false), kept);
    a.bead(["comments", "add", &keys[2], "after the damage"]);
    let kept = answers(&a, &keys, false);
    assert_eq!(answers(&a, &keys, true), kept);
}

#[test]
fn machines_that_sync_answer_alike_with_and_without_their_projections() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "kept-a");
    let mut keys = tracker(&a);
    a.bead(["sync"]);
    let b = crate::rust_only(&sandbox, "kept-b");
    b.bead(["sync"]);
    let _ = answers(&b, &keys, false);
    // each writes and syncs in turn, and once both at once
    let created = b.bead(["create", "from b"]).expect("an issue");
    keys.push(created["id"].as_str().expect("its key").to_owned());
    b.bead(["sync"]);
    a.bead(["sync"]);
    a.bead(["update", &keys[3], "--title", "from a"]);
    b.bead(["update", &keys[3], "--title", "also from b"]);
    a.bead(["sync"]);
    b.bead(["sync"]);
    a.bead(["sync"]);
    for machine in [&a, &b] {
        let kept = answers(machine, &keys, false);
        assert_eq!(answers(machine, &keys, true), kept, "{}", machine.name());
    }
    assert_eq!(answers(&a, &keys, false), answers(&b, &keys, false));
}

#[test]
fn a_bead_killed_as_it_writes_a_projection_leaves_the_last_one() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "kept-a");
    let keys = tracker(&a);
    let _ = answers(&a, &keys, false);
    let last = std::fs::read(projection(&a)).unwrap();
    a.bead(["create", "after", "--offline"]);
    // the next read takes the new record in, and is killed as it writes the projection with it
    let killed = (a.cmd(["list", "--json"]))
        .env("BEAD_FAILPOINTS", "atomic-write:/projection=sigkill")
        .run();
    assert_eq!(killed.returncode(), -9, "{}", killed.stderr);
    assert_eq!(std::fs::read(projection(&a)).unwrap(), last);
    let listed = a.run(["list", "--json"]);
    assert!(listed.stdout.contains("after"), "{}", listed.stdout);
    assert_ne!(std::fs::read(projection(&a)).unwrap(), last);
    assert_eq!(answers(&a, &keys, false), answers(&a, &keys, true));
}
