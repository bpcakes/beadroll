//! The store's projection (roll-lqjj.2), from outside: bead answers alike with it, without it and
//! with a damaged one, its texts kept apart or not (roll-lqjj.6), across machines that sync, and a
//! bead killed as it writes one, or the texts it adds, leaves the last one in place, and the copy
//! it was writing until the next write of that file removes it (roll-lqjj.52); writes in a
//! row leave it to the next read ([`deferred`], roll-lqjj.36); dependency edges and their metadata,
//! kept with the texts, answer alike ([`edges`], roll-lqjj.39); a write-back overtaken by a text
//! file started again writes nothing or every text anew ([`raced`], roll-lqjj.44). Every machine here runs the `bead`
//! this package builds (`rust_only`): the projection is this build's.

use std::os::unix::fs::MetadataExt as _;
use std::path::PathBuf;

use beadroll_test_support::Machine;

mod deferred;
mod edges;
mod raced;

/// The reading commands whose answers are compared.
const READS: [&[&str]; 5] = [
    &["list", "--json"],
    &["list", "-a", "--json"],
    &["ready", "--json"],
    &["blocked", "--json"],
    &["export"],
];

/// The reading commands that answer from the projection's index as well as its state
/// (roll-lqjj.31), compared last: each machine names itself in them.
const HISTORY: [&[&str]; 3] = [
    &["log", "--limit", "0", "--json"],
    &["info", "--json"],
    &["machines", "--json"],
];

fn projection(machine: &Machine) -> PathBuf {
    machine.store().join("projection")
}

fn texts(machine: &Machine) -> PathBuf {
    machine.store().join("projection-texts")
}

/// The temporary files in `machine`'s store, by name: what a bead killed as it writes the
/// projection or its text file leaves beside them.
fn temporaries(machine: &Machine) -> Vec<String> {
    let mut names: Vec<String> = (std::fs::read_dir(machine.store()).unwrap())
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".tmp"))
        .collect();
    names.sort();
    names
}

/// What `machine` answers to the reading commands, to `show` of each of `keys` (`--json`, and as
/// a human reads it), then to the history's; with `replaying`, its projection is removed before
/// each, so that each replays the journal.
fn answers(machine: &Machine, keys: &[String], replaying: bool) -> Vec<String> {
    let mut out = Vec::new();
    let shows: Vec<Vec<&str>> = (keys.iter())
        .flat_map(|key| [vec!["show", key, "--json"], vec!["show", key]])
        .collect();
    let history = HISTORY.iter().map(|args| args.to_vec());
    for args in READS
        .iter()
        .map(|args| args.to_vec())
        .chain(shows)
        .chain(history)
    {
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
    // the same tracker, each machine naming itself in its history's answers
    let (mut of_a, mut of_b) = (answers(&a, &keys, false), answers(&b, &keys, false));
    of_a.truncate(of_a.len() - HISTORY.len());
    of_b.truncate(of_b.len() - HISTORY.len());
    assert_eq!(of_a, of_b);
}

/// Two machines that end claims every way, one closing a claim after another machine took it over
/// (an end too late), with a claim that lost: each prints every issue, its ownership history among
/// it, and the export alike from its projection and replaying the journal, after the projection
/// was written back with the history in its texts and read back (roll-lqjj.40).
#[test]
fn ownership_histories_print_alike_with_and_without_the_projection() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "kept-a");
    let b = crate::rust_only(&sandbox, "kept-b");
    let keys = tracker(&a);
    let (first, second, third) = (&keys[0], &keys[1], &keys[2]);
    a.bead(["release", first, "-r", "for now"]);
    a.bead(["claim", third]);
    a.bead(["sync"]);
    b.bead(["sync"]);
    b.bead(["takeover", third, "-r", "a looked dead"]);
    a.run(["close", third, "-r", "done"]);
    b.bead(["reopen", second]);
    b.bead(["claim", second]);
    b.bead(["claim", first, "--offline"]);
    b.bead(["sync"]);
    a.bead(["sync"]);
    a.bead(["claim", first]);
    crate::commands::ownership::plant_lost_claim(&a, first);
    a.bead(["update", first, "--notes", "a later text"]);
    for machine in [&a, &b] {
        let kept = answers(machine, &keys, false);
        assert_eq!(answers(machine, &keys, true), kept, "{}", machine.name());
    }
    let shown = |key: &str| a.show(key);
    assert!(shown(first)["lost_claims"].truthy(), "{:?}", shown(first));
    assert!(shown(third)["stale"].truthy(), "{:?}", shown(third));
    // the projection a read wrote back answers the same, its histories read in from its texts
    assert!(projection(&a).is_file() && texts(&a).is_file());
    let kept = answers(&a, &keys, false);
    assert_eq!(answers(&a, &keys, false), kept);
    assert_eq!(answers(&a, &keys, true), kept);
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
    // the copy it was writing stays beside the projection, named with its process id, until the
    // next atomic replacement of the projection removes it (roll-lqjj.52)
    let left = temporaries(&a);
    assert!(
        left.len() == 1 && left[0].starts_with(".projection."),
        "{left:?}"
    );
    let listed = a.run(["list", "--json"]);
    assert!(listed.stdout.contains("after"), "{}", listed.stdout);
    assert_ne!(std::fs::read(projection(&a)).unwrap(), last);
    assert_eq!(temporaries(&a), Vec::<String>::new());
    assert_eq!(answers(&a, &keys, false), answers(&a, &keys, true));
}

/// roll-lqjj.52: a bead killed as it starts the text file leaves the copy it was writing beside
/// it, and the next atomic replacement of the text file removes it, as a replacement of the
/// projection removes what a bead killed writing that left.
#[test]
fn a_bead_killed_as_it_starts_its_text_file_leaves_a_copy_the_next_write_removes() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "kept-a");
    let keys = tracker(&a);
    let _ = answers(&a, &keys, false);
    let last = std::fs::read(texts(&a)).unwrap();
    // the next read replays, and is killed as it starts the text file again
    std::fs::remove_file(projection(&a)).unwrap();
    let killed = (a.cmd(["list", "--json"]))
        .env("BEAD_FAILPOINTS", "atomic-write:/projection-texts=sigkill")
        .run();
    assert_eq!(killed.returncode(), -9, "{}", killed.stderr);
    assert_eq!(std::fs::read(texts(&a)).unwrap(), last);
    let left = temporaries(&a);
    assert!(
        left.len() == 1 && left[0].starts_with(".projection-texts."),
        "{left:?}"
    );
    let listed = a.run(["list", "--json"]);
    assert_eq!(listed.returncode(), 0, "{}", listed.stderr);
    assert!(projection(&a).is_file() && texts(&a).is_file());
    assert_eq!(temporaries(&a), Vec::<String>::new());
    assert_eq!(answers(&a, &keys, false), answers(&a, &keys, true));
}

#[test]
fn a_merge_appends_what_another_machines_segment_lacks() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "kept-a");
    let keys = tracker(&a);
    a.bead(["sync"]);
    let b = crate::rust_only(&sandbox, "kept-b");
    b.bead(["sync"]);
    b.bead(["comments", "add", &keys[0], "from b"]);
    a.bead(["sync"]);
    let info = b.bead(["info"]).expect("bead info");
    let machine = info["machine"].as_str().expect("b's machine").to_owned();
    let theirs = a.store().join("events").join(machine).join("000001.jsonl");
    let before = std::fs::metadata(&theirs).unwrap();
    b.bead(["comments", "add", &keys[0], "more from b"]);
    a.bead(["sync"]);
    let after = std::fs::metadata(&theirs).unwrap();
    // the same file, longer: the projection takes in the lines appended alone
    assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
    assert!(after.len() > before.len());
    let kept = answers(&a, &keys, false);
    assert_eq!(answers(&a, &keys, true), kept);
}

#[test]
fn bead_answers_alike_with_its_texts_cut_damaged_or_gone() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "kept-a");
    let keys = tracker(&a);
    a.bead([
        "update",
        &keys[0],
        "--description",
        "why it matters, at length",
        "--notes",
        "and what was tried",
    ]);
    let kept = answers(&a, &keys, false);
    assert!(texts(&a).is_file(), "a read keeps the texts apart");
    let good = std::fs::read(texts(&a)).unwrap();
    let mut flipped = good.clone();
    let last = flipped.len() - 1;
    flipped[last] ^= 0x01;
    // a list needs none of them; a show or an export replays, and writes them again, the same
    // texts as the same bytes
    let mut rebuilt = None;
    for damage in [Some(good[..good.len() / 2].to_vec()), Some(flipped), None] {
        match damage {
            Some(bytes) => std::fs::write(texts(&a), bytes).unwrap(),
            None => std::fs::remove_file(texts(&a)).unwrap(),
        }
        assert_eq!(answers(&a, &keys, false), kept);
        let now = std::fs::read(texts(&a)).unwrap();
        assert_eq!(rebuilt.get_or_insert_with(|| now.clone()), &now);
    }
    // a write about texts that cannot be read in replays
    std::fs::write(texts(&a), &good[..40]).unwrap();
    a.bead(["comments", "add", &keys[0], "after the damage"]);
    let kept = answers(&a, &keys, false);
    assert_eq!(answers(&a, &keys, true), kept);
}

#[test]
fn a_bead_killed_as_it_adds_texts_leaves_the_last_projection() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "kept-a");
    let keys = tracker(&a);
    let _ = answers(&a, &keys, false);
    let last = std::fs::read(projection(&a)).unwrap();
    a.bead(["comments", "add", &keys[0], "written offline", "--offline"]);
    // the next read takes it in, adds its texts, and is killed before the projection is written
    let killed = (a.cmd(["list", "--json"]))
        .env("BEAD_FAILPOINTS", "projection.texts-appended=sigkill")
        .run();
    assert_eq!(killed.returncode(), -9, "{}", killed.stderr);
    assert_eq!(std::fs::read(projection(&a)).unwrap(), last);
    // what it added belongs to no projection: the next read takes the record in again
    let shown = a.run(["show", &keys[0], "--json"]);
    assert!(shown.stdout.contains("written offline"), "{}", shown.stdout);
    assert_eq!(answers(&a, &keys, false), answers(&a, &keys, true));
}

/// roll-lqjj.10: a long text is kept once, in its register, which the field shows, so the text
/// file holds it once: a 100,000-byte description makes about 100 KB of it, and replacing it adds
/// about 100 KB more, where each was written twice; and bead answers alike with and without it.
#[test]
fn a_long_description_is_written_to_the_text_file_once() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "texts-once");
    a.bead(["init", "--prefix", "ex"]);
    let first = sandbox.root().join("first.txt");
    std::fs::write(&first, "d".repeat(100_000)).unwrap();
    let path = first.to_str().unwrap();
    let created = a.bead(["create", "long", "--description-file", path]);
    let key = created.expect("an issue")["id"]
        .as_str()
        .unwrap()
        .to_owned();
    a.bead(["list"]);
    let made = std::fs::metadata(texts(&a)).unwrap().len();
    assert!(made < 101_000, "{made} bytes for one 100,000-byte text");
    let second = sandbox.root().join("second.txt");
    std::fs::write(&second, "e".repeat(100_000)).unwrap();
    a.bead([
        "update",
        &key,
        "--description-file",
        second.to_str().unwrap(),
    ]);
    a.bead(["show", &key]);
    let grown = std::fs::metadata(texts(&a)).unwrap().len() - made;
    assert!(
        grown < 101_000,
        "{grown} bytes added for one 100,000-byte text"
    );
    let keys = [key];
    assert_eq!(answers(&a, &keys, false), answers(&a, &keys, true));
    let shown = a.show(&keys[0]);
    assert_eq!(shown["description"].as_str().map(str::len), Some(100_000));
}
