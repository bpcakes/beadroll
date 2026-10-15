//! A write leaves the stored projection as it found it while the records after it are few
//! (roll-lqjj.36, X17): the next read takes them in and writes it back, and a write writes it back
//! only once they hold more than a 64th of its bytes. Whatever kills a write between its steps,
//! and whatever a read writing the projection back meets, bead answers as the journal replayed.

use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use beadroll_test_support::Machine;

use super::{answers, projection};
use crate::hardening::gate::{Gate, beside};

/// How many times the records after the stored projection may go into its bytes before a write
/// writes it back (`TAIL` in `beadroll-store`'s projection).
const TAIL: u64 = 64;

/// A tracker of `n` issues imported from a Beads export, its projection kept by a read: large
/// enough that one write's record is a small part of it. The keys `answers` shows.
fn imported(machine: &Machine, n: usize) -> Vec<String> {
    machine.bead(["init", "--prefix", "ex"]);
    let export = machine.home().join("export.jsonl");
    let lines: Vec<String> = (1..=n)
        .map(|i| {
            format!(
                "{{\"id\": \"ex-{i}\", \"title\": \"issue {i}\", \"status\": \"open\", \
                 \"priority\": 2, \"issue_type\": \"task\"}}"
            )
        })
        .collect();
    std::fs::write(&export, lines.join("\n") + "\n").unwrap();
    machine.bead(["import", export.to_str().unwrap(), "--offline"]);
    let keys = vec!["ex-1".to_owned(), "ex-2".to_owned(), format!("ex-{n}")];
    let _ = answers(machine, &keys, false);
    assert!(projection(machine).is_file(), "a read keeps a projection");
    keys
}

/// How many bytes the store's journal files hold: its segments and attempt files, what the
/// projection keeps the length of.
fn journal_bytes(machine: &Machine) -> u64 {
    fn walk(dir: &Path) -> u64 {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return 0;
        };
        (entries.flatten())
            .map(|entry| {
                let path = entry.path();
                match entry.metadata() {
                    Ok(meta) if meta.is_dir() => walk(&path),
                    Ok(meta) => meta.len(),
                    Err(_) => 0,
                }
            })
            .sum()
    }
    walk(&machine.store().join("events"))
}

/// A comment written offline: only the store writes.
fn comment(machine: &Machine, key: &str, text: &str) {
    machine.bead(["comments", "add", key, text, "--offline"]);
}

/// How long a write may take beside a read held at a gate before the test calls it stuck.
const LIMIT: Duration = Duration::from_secs(30);

/// A comment written offline that has [`LIMIT`] to finish, killed otherwise: what it printed to
/// standard error when it failed or was stuck. A write that waited for the held read to finish
/// would wait as long as the gate, and the test with it.
fn comment_within(machine: &Machine, key: &str, text: &str) -> Result<(), String> {
    let mut child = (machine
        .cmd(["comments", "add", key, text, "--offline"])
        .command())
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::piped())
    .spawn()
    .unwrap();
    let deadline = Instant::now() + LIMIT;
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            child.kill().unwrap();
            let stuck = child.wait_with_output().unwrap();
            return Err(format!(
                "still waiting after {LIMIT:?}: {}",
                String::from_utf8_lossy(&stuck.stderr)
            ));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let done = child.wait_with_output().unwrap();
    match done.status.success() {
        true => Ok(()),
        false => Err(String::from_utf8_lossy(&done.stderr).into_owned()),
    }
}

/// A write finds the stored projection and the records after it; it writes the projection back
/// when those records hold more than a 64th of its bytes, and leaves it otherwise, however many
/// writes come in a row; the read after them takes every record in and writes it back.
#[test]
fn writes_in_a_row_leave_the_projection_until_the_records_after_it_are_many() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "deferred-row");
    let keys = imported(&a, 300);
    let mut stored = std::fs::read(projection(&a)).unwrap();
    // the journal's bytes when the stored projection was made
    let mut made = journal_bytes(&a);
    let (mut left, mut kept) = (0, 0);
    for n in 0..16 {
        let before = journal_bytes(&a);
        comment(&a, &keys[0], &format!("note {n}"));
        let now = std::fs::read(projection(&a)).unwrap();
        if (before - made) * TAIL > stored.len() as u64 {
            assert_ne!(now, stored, "write {n}: the records before it are many");
            // made from the journal as the write found it, before its own record
            (stored, made, kept) = (now, before, kept + 1);
        } else {
            assert_eq!(now, stored, "write {n}: the records before it are few");
            left += 1;
        }
    }
    assert!(left >= 8 && kept >= 2, "{left} left, {kept} written back");
    assert!(
        journal_bytes(&a) > made,
        "records after the stored projection"
    );
    let read = answers(&a, &keys, false);
    assert_ne!(
        std::fs::read(projection(&a)).unwrap(),
        stored,
        "the read wrote it back"
    );
    assert_eq!(answers(&a, &keys, true), read);
    let shown = a.run(["show", &keys[0], "--json"]);
    assert!(shown.stdout.contains("note 15"), "{}", shown.stdout);
}

/// A write killed as it reads the projection, as it reads the records after it, halfway through
/// appending its own, or as it writes the projection back leaves the stored one as it was, and bead
/// answers as the journal replayed; a write after them goes through, and a read takes it in. The
/// first three kill it after one write left its record, then after as many as leave most of what a
/// write takes in before it writes the projection back.
#[test]
fn a_write_killed_between_its_steps_leaves_the_projection_it_found() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "deferred-killed");
    let keys = imported(&a, 300);
    for long in [false, true] {
        for failpoints in [
            "projection.read=sigkill",
            "journal.before-read=sigkill",
            "append.mid=torn-sigkill",
        ] {
            // a record after the stored projection, which the reads below then take in; the first
            // after `append.mid` cuts the half line it left
            comment(&a, &keys[0], "left to the next read");
            if long {
                // written back, then three quarters of what a write leaves to the next read
                let _ = answers(&a, &keys, false);
                let (written, made) = (std::fs::read(projection(&a)).unwrap(), journal_bytes(&a));
                while (journal_bytes(&a) - made) * TAIL * 4 <= written.len() as u64 * 3 {
                    comment(&a, &keys[0], "one more");
                }
                assert_eq!(std::fs::read(projection(&a)).unwrap(), written);
                assert!(
                    (journal_bytes(&a) - made) * TAIL <= written.len() as u64,
                    "the killed write would write it back"
                );
            }
            let stored = std::fs::read(projection(&a)).unwrap();
            let killed = (a.cmd(["comments", "add", &keys[1], "killed-mid-write", "--offline"]))
                .env("BEAD_FAILPOINTS", failpoints)
                .run();
            assert_eq!(killed.returncode(), -9, "{failpoints}: {}", killed.stderr);
            assert_eq!(
                std::fs::read(projection(&a)).unwrap(),
                stored,
                "{failpoints}, long {long}"
            );
            assert_eq!(
                answers(&a, &keys, false),
                answers(&a, &keys, true),
                "{failpoints}, long {long}"
            );
        }
    }
    // records after the stored projection many enough that the next write writes it back, which is
    // killed as it does; the first of them cuts the half line left above
    comment(&a, &keys[0], "after the torn line");
    let _ = answers(&a, &keys, false);
    let (stored, made) = (std::fs::read(projection(&a)).unwrap(), journal_bytes(&a));
    while (journal_bytes(&a) - made) * TAIL <= stored.len() as u64 {
        comment(&a, &keys[0], "one more");
    }
    let segments = journal_bytes(&a);
    let killed = (a.cmd(["comments", "add", &keys[1], "killed-mid-write", "--offline"]))
        .env("BEAD_FAILPOINTS", "atomic-write:/projection=sigkill")
        .run();
    assert_eq!(killed.returncode(), -9, "{}", killed.stderr);
    assert_eq!(std::fs::read(projection(&a)).unwrap(), stored);
    assert_eq!(journal_bytes(&a), segments, "killed before its own record");
    comment(&a, &keys[1], "after the kills");
    let read = answers(&a, &keys, false);
    assert_eq!(answers(&a, &keys, true), read);
    let shown = a.run(["show", &keys[1], "--json"]);
    assert!(shown.stdout.contains("after the kills"), "{}", shown.stdout);
    assert!(
        !shown.stdout.contains("killed-mid-write"),
        "{}",
        shown.stdout
    );
}

/// A read that writes the projection back, held just before it renames it into place while two
/// writes go through: the writes leave the stored projection as they found it, the read's is made
/// from the journal before them, and the next read takes both in.
#[test]
fn writes_while_a_read_writes_the_projection_back_are_taken_in_by_the_next_read() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "deferred-race");
    let keys = imported(&a, 300);
    comment(&a, &keys[0], "before the read");
    let stored = std::fs::read(projection(&a)).unwrap();
    let gate = Gate::new(&sandbox, "deferred-race");
    let failpoints = format!("atomic-write:/projection={}", gate.action());
    let (read, met) = beside(
        || {
            (a.cmd(["list", "--json"]))
                .env("BEAD_FAILPOINTS", &failpoints)
                .run()
        },
        |finished| {
            gate.meet(finished, || {
                // a write that waited for the read would have the test wait out the gate
                comment_within(&a, &keys[1], "while the read waits").unwrap();
                comment_within(&a, &keys[2], "this one too").unwrap();
                std::fs::read(projection(&a)).unwrap()
            })
        },
    );
    let during = met.unwrap_or_else(|| panic!("the read wrote nothing back: {}", read.stderr));
    assert_eq!(read.returncode(), 0, "{}", read.stderr);
    assert!(read.stdout.contains("ex-1"), "{}", read.stdout);
    assert_eq!(during, stored, "the writes left the stored projection");
    let written = std::fs::read(projection(&a)).unwrap();
    assert_ne!(written, stored, "the read's, made before the writes");
    let answered = answers(&a, &keys, false);
    assert_ne!(std::fs::read(projection(&a)).unwrap(), written);
    assert_eq!(answers(&a, &keys, true), answered);
    for (key, text) in [
        (&keys[1], "while the read waits"),
        (&keys[2], "this one too"),
    ] {
        let shown = a.run(["show", key, "--json"]);
        assert!(shown.stdout.contains(text), "{}", shown.stdout);
    }
}
