//! A write-back that another process overtakes by starting the text file again (roll-lqjj.44): a
//! projection read back places its issues' texts in the text file it was read with, and so does
//! every record a write-back writes or copies of it. When another process starts the text file
//! again between the read and the write-back, the write-back finds another generation there: it
//! writes nothing, or, when it has every text in it, every text anew in a text file of its own and
//! every record with them. Either way bead answers as the journal replayed. One whose text file
//! another process grew under it starts a text file of its own and writes every record anew too.

use std::os::unix::fs::MetadataExt as _;

use beadroll_test_support::{Machine, Run, Sandbox};

use super::{answers, left, projection, texts, tracker};
use crate::hardening::gate::{Gate, beside};

/// The generation the text file was started with: its header's second sixteen bytes.
fn generation(machine: &Machine) -> Vec<u8> {
    std::fs::read(texts(machine)).unwrap()[16..32].to_vec()
}

/// The projection and the text file as they are: their bytes and inodes.
fn files(machine: &Machine) -> (Vec<u8>, Vec<u8>, u64, u64) {
    let inode = |path| std::fs::metadata(path).unwrap().ino();
    (
        std::fs::read(projection(machine)).unwrap(),
        std::fs::read(texts(machine)).unwrap(),
        inode(projection(machine)),
        inode(texts(machine)),
    )
}

/// A comment on `key`, written offline so that the stored projection is left without it, then
/// `list`, which takes it in and stops before it writes the projection back while another process
/// replays the journal and starts the text file again (at `gate`, its name): what the list printed,
/// and the files as the other left them.
fn overtaken(
    sandbox: &Sandbox,
    machine: &Machine,
    key: &str,
    gate: &str,
) -> (Run, (Vec<u8>, Vec<u8>, u64, u64)) {
    let _ = machine.run(["list", "--json"]); // the projection written back, nothing after it
    left(
        machine,
        &["comments", "add", key, "taken in by the list", "--offline"],
    );
    let read = generation(machine);
    let gate = Gate::new(sandbox, gate);
    let failpoints = format!("projection.before-save={}", gate.action());
    let (list, met) = beside(
        || {
            (machine.cmd(["list", "--json"]))
                .env("BEAD_FAILPOINTS", &failpoints)
                .run()
        },
        |finished| {
            gate.meet(finished, || {
                std::fs::remove_file(projection(machine)).unwrap();
                let replayed = machine.run(["list", "--json"]);
                assert_eq!(replayed.returncode(), 0, "{}", replayed.stderr);
                assert_ne!(generation(machine), read, "the text file was started again");
                files(machine)
            })
        },
    );
    let left = met.unwrap_or_else(|| panic!("the list wrote nothing back: {}", list.stderr));
    assert_eq!(list.returncode(), 0, "{}", list.stderr);
    (list, left)
}

#[test]
fn a_write_back_overtaken_by_a_text_file_started_again_writes_nothing_or_every_text_anew() {
    let sandbox = crate::sandbox();
    // Texts it does not have: three issues, one of which the list reads in. It writes nothing.
    let a = crate::rust_only(&sandbox, "raced-a");
    let keys = tracker(&a);
    let (list, left) = overtaken(&sandbox, &a, &keys[0], "raced-a");
    assert_eq!(files(&a), left, "the overtaken write-back wrote nothing");
    let replayed = answers(&a, &keys, true);
    assert_eq!(list.stdout, replayed[0]);
    assert_eq!(answers(&a, &keys, false), replayed);
    // Every text in it: one issue, which the list reads in, on a remote of its own. It starts a
    // text file of its own.
    let sandbox = crate::sandbox();
    let b = crate::rust_only(&sandbox, "raced-b");
    b.bead(["init", "--prefix", "ex"]);
    let only = b.bead(["create", "only"]).expect("an issue");
    let only = vec![only["id"].as_str().expect("its key").to_owned()];
    let (list, left) = overtaken(&sandbox, &b, &only[0], "raced-b");
    let now = files(&b);
    assert_ne!((now.2, now.3), (left.2, left.3), "both files written again");
    let replayed = answers(&b, &only, true);
    assert_eq!(list.stdout, replayed[0]);
    assert_eq!(answers(&b, &only, false), replayed);
}

/// A write-back whose text file another process grew under it, past what it reads of it by more
/// than a mebibyte: it reads every text in and starts a text file of its own, and so writes every
/// record anew, with its texts' place in that file, those of the issues it did not change too. The
/// next read takes in what the other process wrote, reading the texts of the issue it is about
/// where the write-back placed them: it extends the projection and appends to that text file,
/// where a text read at a place of the old file would fail its sum and have the journal replayed,
/// and the file started again.
#[test]
fn a_write_back_whose_text_file_grew_under_it_writes_every_record_anew() {
    let sandbox = crate::sandbox();
    let c = crate::rust_only(&sandbox, "raced-c");
    let keys = tracker(&c);
    let _ = c.run(["list", "--json"]);
    left(
        &c,
        &[
            "comments",
            "add",
            &keys[0],
            "taken in by the list",
            "--offline",
        ],
    );
    let long = sandbox.root().join("long.txt");
    std::fs::write(&long, "x".repeat(1_200_000)).unwrap();
    let long = long.to_str().unwrap().to_owned();
    let gate = Gate::new(&sandbox, "raced-c");
    let failpoints = format!("projection.before-save={}", gate.action());
    let (list, grown) = beside(
        || {
            (c.cmd(["list", "--json"]))
                .env("BEAD_FAILPOINTS", &failpoints)
                .run()
        },
        |finished| {
            gate.meet(finished, || {
                // a description longer than a mebibyte on an issue the list leaves as it is, and
                // a read that writes it back, appending it to the list's text file
                c.bead(["update", &keys[2], "--description-file", &long, "--offline"]);
                let _ = c.run(["list", "--json"]);
                std::fs::metadata(texts(&c)).unwrap().len()
            })
        },
    );
    let grown = grown.unwrap_or_else(|| panic!("the list wrote nothing back: {}", list.stderr));
    assert_eq!(list.returncode(), 0, "{}", list.stderr);
    assert!(grown > 1 << 20, "the text file grew to {grown} bytes");
    let started = files(&c);
    assert!(started.1.len() < 1 << 20, "a text file of its own");
    let shown = c.run(["show", &keys[2], "--json"]);
    assert_eq!(shown.returncode(), 0, "{}", shown.stderr);
    assert_eq!(
        files(&c).3,
        started.3,
        "the texts read where the write-back placed them"
    );
    let replayed = answers(&c, &keys, true);
    assert_eq!(answers(&c, &keys, false), replayed);
}
