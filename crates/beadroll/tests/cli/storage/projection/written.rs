//! The read after a write (roll-lqjj.51): a write writes its own projection back once it has let go
//! of the store lock, so that the read after it, whatever the write was, offline or published at
//! once, a local edit or an ownership command, finds the projection current and applies nothing,
//! and prints what the journal replayed prints, byte for byte. What the write writes back is what a
//! read writes back from the projection the write found. A write killed as it writes back has gone
//! through, and the next read takes its record in; a write-back overtaken by another write's is
//! one the next read extends. A write that writes nothing writes nothing back.

use std::os::unix::fs::MetadataExt as _;

use beadroll_test_support::Machine;

use super::{projection, texts, tracker};
use crate::hardening::gate::{Gate, beside};

/// The writes made in turn, on the tracker's keys (`{0}`, `{1}`, `{2}`), each with the read after
/// it.
const WRITES: [(&[&str], &[&str]); 12] = [
    (
        &["comments", "add", "{0}", "offline", "--offline"],
        &["show", "{0}", "--json"],
    ),
    (
        &["comments", "add", "{1}", "published"],
        &["show", "{1}", "--json"],
    ),
    (&["create", "made", "--offline"], &["list", "--json"]),
    (
        &["update", "{2}", "--title", "renamed", "--offline"],
        &["show", "{2}", "--json"],
    ),
    (
        &["dep", "add", "{2}", "{0}", "--offline"],
        &["show", "{2}", "--json"],
    ),
    (
        &["close", "{2}", "-r", "done", "--offline"],
        &["show", "{2}", "--json"],
    ),
    (&["reopen", "{2}", "--offline"], &["ready", "--json"]),
    (&["claim", "{2}"], &["show", "{2}", "--json"]),
    (&["release", "{2}"], &["show", "{2}", "--json"]),
    (&["claim", "{2}", "--offline"], &["show", "{2}", "--json"]),
    (&["update", "{1}", "--add-label", "later"], &["show", "{1}"]),
    (
        &["delete", "{2}", "--force", "--offline"],
        &["list", "-a", "--json"],
    ),
];

/// A read that fails (97) if it applies records appended after the stored projection.
const APPLIES_NOTHING: &str = "projection.extended=exit(97)";

/// `args` with the keys put in.
fn filled(args: &[&str], keys: &[String]) -> Vec<String> {
    (args.iter())
        .map(|arg| {
            (0..keys.len()).fold((*arg).to_owned(), |arg, at| {
                arg.replace(&format!("{{{at}}}"), &keys[at])
            })
        })
        .collect()
}

/// What `machine` prints to `args`, the projection removed first when `replaying`.
fn printed(machine: &Machine, args: &[String], replaying: bool) -> String {
    if replaying {
        let _ = std::fs::remove_file(projection(machine));
    }
    let run = machine.run(args);
    assert_eq!(run.returncode(), 0, "{args:?}: {}", run.stderr);
    run.stdout
}

/// The projection and its text file, as their bytes.
fn stored(machine: &Machine) -> (Vec<u8>, Vec<u8>) {
    let read = |path| std::fs::read(path).unwrap_or_default();
    (read(projection(machine)), read(texts(machine)))
}

/// Of the projection and text file `files`, what holds the projection's state and index: the
/// header's lengths and sums of both, the parts themselves and the text file. What it was made from
/// is left out: a read keeps no sum of an attempt file it finds unchanged, where a write keeps the
/// one the projection it found had.
fn projected(files: &(Vec<u8>, Vec<u8>)) -> (&[u8], &[u8], &[u8], &[u8]) {
    let bytes = &files.0;
    let made = u64::from_le_bytes(bytes[24..32].try_into().unwrap()) as usize;
    (
        &bytes[32..48],
        &bytes[64..96],
        &bytes[96 + made..],
        &files.1,
    )
}

/// Puts back the projection and the text file `files`.
fn restore(machine: &Machine, files: &(Vec<u8>, Vec<u8>)) {
    std::fs::write(projection(machine), &files.0).unwrap();
    std::fs::write(texts(machine), &files.1).unwrap();
}

/// What `machine` prints to `args` when it applies nothing; it fails when it would.
fn applying_nothing(machine: &Machine, args: &[String]) -> String {
    let run = (machine.cmd(args))
        .env("BEAD_FAILPOINTS", APPLIES_NOTHING)
        .run();
    assert_eq!(
        run.returncode(),
        0,
        "{args:?} applied records: {}",
        run.stderr
    );
    run.stdout
}

#[test]
fn the_read_after_a_write_prints_what_the_journal_replayed_prints() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "written");
    let keys = tracker(&a);
    for (write, read) in WRITES {
        let (write, read) = (filled(write, &keys), filled(read, &keys));
        let found = stored(&a);
        let run = a.run(&write);
        assert_eq!(run.returncode(), 0, "{write:?}: {}", run.stderr);
        let written = stored(&a);
        assert_ne!(written.0, found.0, "{write:?} wrote its projection back");
        let after = applying_nothing(&a, &read);
        // what a read writes back of the projection the write found, with the write's record
        restore(&a, &found);
        let _ = printed(&a, &read, false);
        assert!(
            projected(&stored(&a)) == projected(&written),
            "{write:?}: not what a read writes back"
        );
        assert_eq!(after, printed(&a, &read, true), "{write:?}, then {read:?}");
    }
}

/// A write killed once it has let go of the store lock, before or as it writes its projection back,
/// has gone through: the stored projection is the one it found, and the next read takes its record
/// in and answers as the journal replayed.
#[test]
fn a_write_killed_as_it_writes_its_projection_back_has_gone_through() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "written-killed");
    let keys = tracker(&a);
    let show = filled(&["show", "{0}", "--json"], &keys);
    for (n, failpoints) in ["write.save=sigkill", "atomic-write:/projection=sigkill"]
        .into_iter()
        .enumerate()
    {
        let _ = printed(&a, &show, false);
        let found = stored(&a);
        let text = format!("killed {n}");
        let killed = (a.cmd(["comments", "add", &keys[0], &text, "--offline"]))
            .env("BEAD_FAILPOINTS", failpoints)
            .run();
        assert_eq!(killed.returncode(), -9, "{failpoints}: {}", killed.stderr);
        assert!(
            stored(&a).0 == found.0,
            "{failpoints}: the projection it found"
        );
        let extends = (a.cmd(&show)).env("BEAD_FAILPOINTS", APPLIES_NOTHING).run();
        assert_eq!(
            extends.returncode(),
            97,
            "{failpoints}: its record taken in"
        );
        let after = printed(&a, &show, false);
        assert!(after.contains(&text), "{after}");
        assert_eq!(after, printed(&a, &show, true), "{failpoints}");
    }
}

/// Two writes, the first held once it has let go of the store lock and before it writes its
/// projection back while the second goes through, writes its own back, and the first then writes
/// back the one it made last: the read after both takes the second's record in, writes the
/// projection back, and answers as the journal replayed; the read after that applies nothing.
#[test]
fn a_write_back_overtaken_by_another_writes_is_extended_by_the_next_read() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "written-race");
    let keys = tracker(&a);
    let show = filled(&["show", "{0}", "--json"], &keys);
    let gate = Gate::new(&sandbox, "written-race");
    let failpoints = format!("write.save={}", gate.action());
    let (first, second) = beside(
        || {
            (a.cmd(["comments", "add", &keys[0], "the first", "--offline"]))
                .env("BEAD_FAILPOINTS", &failpoints)
                .run()
        },
        |finished| {
            gate.meet(finished, || {
                let second = a.run(["comments", "add", &keys[0], "the second", "--offline"]);
                assert_eq!(second.returncode(), 0, "{}", second.stderr);
                stored(&a)
            })
        },
    );
    let second = second.unwrap_or_else(|| panic!("the first wrote nothing back: {}", first.stderr));
    assert_eq!(first.returncode(), 0, "{}", first.stderr);
    assert!(stored(&a) != second, "the first's write-back came last");
    let extends = (a.cmd(&show)).env("BEAD_FAILPOINTS", APPLIES_NOTHING).run();
    assert_eq!(extends.returncode(), 97, "the second's record taken in");
    let after = printed(&a, &show, false);
    for text in ["the first", "the second"] {
        assert!(after.contains(text), "{after}");
    }
    assert_eq!(applying_nothing(&a, &show), after);
    assert_eq!(after, printed(&a, &show, true));
}

/// A write that writes nothing, a reopen of an issue that is open, writes nothing back either: its
/// projection holds no event the stored one lacks.
#[test]
fn a_write_that_writes_nothing_writes_nothing_back() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "written-nothing");
    let keys = tracker(&a);
    let _ = a.run(["list", "--json"]);
    let inode = |machine: &Machine| std::fs::metadata(projection(machine)).unwrap().ino();
    let (found, at) = (stored(&a), inode(&a));
    let run = a.run(["reopen", &keys[2], "--offline"]);
    assert_eq!(run.returncode(), 0, "{}", run.stderr);
    assert!(stored(&a) == found && inode(&a) == at, "written back");
}
