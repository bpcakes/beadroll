//! The bookmark follows delivery, and its separate lock never stops journal writers.

use std::fs::{File, OpenOptions};
use std::os::unix::fs::PermissionsExt as _;
use std::process::Stdio;

use beadroll_test_support::{Machine, code, parse_json};

use super::{create, events, recap};

#[test]
fn a_read_error_keeps_the_bookmark_and_prints_no_partial_answer() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let key = create(&a, "Already read");
    recap(&a);
    a.bead(["comments", "add", &key, "Unread"]);
    let cursor = a.home().join(".recap.json");
    let before = std::fs::read(&cursor).unwrap();
    let segment = super::segments(&a.store()).pop().unwrap();
    let original = std::fs::read(&segment).unwrap();
    std::fs::remove_file(&segment).unwrap();
    std::fs::create_dir(&segment).unwrap();
    let failed = a.run(["recap", "--json"]);
    assert_eq!(failed.returncode(), 1);
    assert_eq!(code(&failed), "IO");
    assert!(failed.stdout.is_empty());
    assert_eq!(std::fs::read(&cursor).unwrap(), before);
    std::fs::remove_dir(&segment).unwrap();
    std::fs::write(segment, original).unwrap();
    assert_eq!(events(&recap(&a)).len(), 1);
}
use crate::hardening::gate::{Gate, beside};

#[test]
fn failed_output_and_closed_pipes_leave_the_cursor_unchanged() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let key = create(&a, "Already read");
    recap(&a);
    a.bead(["comments", "add", &key, "Still unread"]);
    let path = a.home().join(".recap.json");
    let before = std::fs::read(&path).unwrap();
    for json in [false, true] {
        let args: &[&str] = if json {
            &["recap", "--json"]
        } else {
            &["recap"]
        };
        let (reader, writer) = std::io::pipe().unwrap();
        drop(reader);
        let gone = a
            .cmd(args)
            .command()
            .stdin(Stdio::null())
            .stdout(writer)
            .stderr(Stdio::piped())
            .output()
            .unwrap();
        assert!(gone.status.success());
        assert!(gone.stderr.is_empty());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        // A read-only descriptor is a real output error on Linux and macOS alike.
        let failed = a
            .cmd(args)
            .command()
            .stdin(Stdio::null())
            .stdout(File::open(&path).unwrap())
            .stderr(Stdio::piped())
            .output()
            .unwrap();
        assert_eq!(failed.status.code(), Some(1));
        assert!(!failed.stderr.is_empty());
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
    assert_eq!(events(&recap(&a)).len(), 1);
    assert!(events(&recap(&a)).is_empty());
}

#[test]
fn failed_cursor_writes_and_interrupted_reads_are_retriable() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let key = create(&a, "Read once");
    recap(&a);
    a.bead(["comments", "add", &key, "Retry me"]);
    let path = a.home().join(".recap.json");
    let before = std::fs::read(&path).unwrap();
    let stopped = a
        .cmd(["recap", "--json"])
        .env("BEAD_FAILPOINTS", "journal.after-enumerate=exit(98)")
        .run();
    assert_eq!(stopped.returncode(), 98);
    assert!(stopped.stdout.is_empty());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let failed = a
        .cmd(["recap", "--json"])
        .env("BEAD_FAILPOINTS", "atomic-write:.recap.json=errno(ENOSPC)")
        .run();
    assert_eq!(failed.returncode(), 1);
    assert_eq!(code(&failed), "IO");
    assert!(failed.stderr.contains(".recap.json"));
    assert_eq!(events(&parse_json(&failed.stdout).unwrap()).len(), 1);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let stopped = a
        .cmd(["recap", "--json"])
        .env("BEAD_FAILPOINTS", "atomic-write:.recap.json=exit(99)")
        .run();
    assert_eq!(stopped.returncode(), 99);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(events(&recap(&a)).len(), 1);
    assert!(events(&recap(&a)).is_empty());
    assert_eq!(
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn overlapping_reads_serialize_while_journal_writes_continue() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let key = create(&a, "First answer");
    let gate = Gate::new(&sandbox, "recap-save");
    let fault = format!("atomic-write:.recap.json={}", gate.action());
    let (first, second) = beside(
        || {
            a.cmd(["recap", "--json"])
                .env("BEAD_FAILPOINTS", &fault)
                .run()
        },
        |finished| {
            gate.meet(finished, || {
                let lock = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(a.home().join(".recap.lock"))
                    .unwrap();
                assert!(
                    matches!(lock.try_lock(), Err(std::fs::TryLockError::WouldBlock)),
                    "the bookmark lock must span output and the atomic save"
                );
                a.bead(["comments", "add", &key, "During the first read"]);
                a.cmd(["recap", "--json"])
                    .command()
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .unwrap()
            })
        },
    );
    assert_eq!(first.returncode(), 0, "{}", first.stderr);
    assert_eq!(events(&parse_json(&first.stdout).unwrap()).len(), 1);
    let second = second
        .expect("the cursor-write failpoint fired")
        .wait_with_output()
        .unwrap();
    assert!(second.status.success(), "{:?}", second.stderr);
    let answer = parse_json(&String::from_utf8(second.stdout).unwrap()).unwrap();
    let rows = events(&answer);
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0]["summary"].as_str(),
        Some("comment: During the first read")
    );
    assert!(events(&recap(&a)).is_empty());
}
