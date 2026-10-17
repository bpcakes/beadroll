//! When a recap marks what it showed as seen: once its answer is written, and only where a person
//! reads it, a terminal, unless `--advance` asks; never for a reader that went away, nor before a
//! failed save it reports. Two recaps at once lose neither's mark, and one waiting on its reader
//! holds no other up.

use std::io::Read as _;
use std::process::Stdio;
use std::time::{Duration, Instant};

use beadroll_test_support::{Machine, code, parse_json};

use super::{create, each, events, recap};
use crate::hardening::g59::run_on_terminal;
use crate::hardening::gate::{Gate, beside};

#[test]
fn only_output_to_a_terminal_marks_activity_seen_unless_advance_asks() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    create(&a, "One");
    let cursor = a.home().join(".recap.json");
    // a pipe, --json (an agent's run) and --no-advance on a terminal only show it
    assert!(a.run(["recap"]).stdout.contains("One"));
    assert_eq!(each(&a.bead(["recap"]).unwrap(), "title"), ["One"]);
    for args in [&["recap", "--no-advance"][..], &["recap", "--json"]] {
        let (status, shown) = run_on_terminal(a.cmd(args).command());
        assert_eq!(status, 0, "{shown}");
        assert!(shown.contains("One"), "{shown}");
    }
    assert!(!cursor.exists());
    // a terminal marks what it showed
    let (status, shown) = run_on_terminal(a.cmd(["recap"]).command());
    assert_eq!(status, 0, "{shown}");
    assert!(shown.contains("One"), "{shown}");
    assert!(events(&a.bead(["recap"]).unwrap()).is_empty());
    // and --advance what a pipe takes in
    create(&a, "Two");
    assert!(a.run(["recap", "--advance"]).stdout.contains("Two"));
    assert!(events(&a.bead(["recap"]).unwrap()).is_empty());
    let both = a.run(["recap", "--advance", "--no-advance", "--json"]);
    assert_eq!((both.returncode(), code(&both)), (2, "USAGE".to_owned()));
}

#[test]
fn a_reader_that_went_away_leaves_what_it_missed_unseen() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let key = create(&a, "Read");
    recap(&a);
    a.bead(["comments", "add", &key, "Never read"]);
    let path = a.home().join(".recap.json");
    let before = std::fs::read(&path).unwrap();
    for args in [
        &["recap", "--advance"][..],
        &["recap", "--advance", "--json"],
    ] {
        let (reader, writer) = std::io::pipe().unwrap();
        drop(reader);
        let gone = (a.cmd(args).command())
            .stdin(Stdio::null())
            .stdout(writer)
            .stderr(Stdio::piped())
            .output()
            .unwrap();
        assert!(gone.status.success(), "{gone:?}");
        assert!(gone.stderr.is_empty(), "{gone:?}");
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
    assert_eq!(each(&recap(&a), "summary"), ["comment: Never read"]);
}

#[test]
fn a_cursor_that_cannot_be_saved_says_so_and_the_activity_shows_again() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let key = create(&a, "Read once");
    recap(&a);
    a.bead(["comments", "add", &key, "Shown, not saved"]);
    let path = a.home().join(".recap.json");
    let before = std::fs::read(&path).unwrap();
    let failing = |fault: &str| {
        a.cmd(["recap", "--advance", "--json"])
            .env("BEAD_FAILPOINTS", fault)
            .run()
    };
    let full = failing("atomic-write:.recap.json=errno(ENOSPC)");
    assert_eq!((full.returncode(), code(&full)), (1, "IO".to_owned()));
    assert!(full.stderr.contains(".recap.json"), "{}", full.stderr);
    let answer = parse_json(&full.stdout).unwrap();
    assert_eq!(each(&answer, "summary"), ["comment: Shown, not saved"]);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let stopped = failing("atomic-write:.recap.json=exit(99)");
    assert_eq!(stopped.returncode(), 99, "{}", stopped.stderr);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let stopped = failing("journal.after-enumerate=exit(98)");
    assert_eq!(stopped.returncode(), 98, "{}", stopped.stderr);
    assert!(stopped.stdout.is_empty());
    assert_eq!(each(&recap(&a), "summary"), ["comment: Shown, not saved"]);
    assert!(events(&recap(&a)).is_empty());
    let mode =
        std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&path).unwrap().permissions());
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn two_recaps_at_once_lose_neither_ones_mark() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let key = create(&a, "First answer");
    let gate = Gate::new(&sandbox, "recap-save");
    let fault = format!("recap.before-save={}", gate.action());
    let (first, second) = beside(
        || {
            a.cmd(["recap", "--advance", "--json"])
                .env("BEAD_FAILPOINTS", &fault)
                .run()
        },
        |finished| {
            // the first has written its answer and has yet to save it
            gate.meet(finished, || {
                a.bead(["comments", "add", &key, "While the first was saving"]);
                a.cmd(["recap", "--advance", "--json"]).run()
            })
        },
    );
    assert_eq!(first.returncode(), 0, "{}", first.stderr);
    assert_eq!(
        each(&parse_json(&first.stdout).unwrap(), "title"),
        ["First answer"]
    );
    let second = second.expect("the first recap reached its save");
    assert_eq!(second.returncode(), 0, "{}", second.stderr);
    let saw = parse_json(&second.stdout).unwrap();
    assert_eq!(each(&saw, "op"), ["create", "comment"]);
    // the first saved last, and kept the second's mark, which saw more
    assert!(events(&recap(&a)).is_empty());
}

#[test]
fn a_recap_waiting_on_its_reader_holds_no_other_up() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let key = create(&a, "Long discussion");
    let long = "x".repeat(400_000);
    a.cmd(["comments", "add", &key, "-f", "-"])
        .stdin(long)
        .json();
    // full: the comment's text in its summary
    let mut slow = (a.cmd(["recap", "--advance", "--format", "full"]).command())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // it is writing its answer, which no pipe holds whole, and waits for its reader
    let mut output = slow.stdout.take().unwrap();
    let mut first = [0u8; 1];
    output.read_exact(&mut first).unwrap();
    let mut other = (a.cmd(["recap", "--advance", "--json"]).command())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    let status = loop {
        if let Some(status) = other.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "the other recap waited on the first's reader"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(status.success());
    let mut rest = Vec::new();
    output.read_to_end(&mut rest).unwrap();
    assert!(slow.wait().unwrap().success());
    assert!(events(&recap(&a)).is_empty());
}
