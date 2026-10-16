//! What bead holds on its heap, counted as `cargo xtask bead` counts it: the most it held at once
//! (`heap peak:`) and what it still held when it returned (`heap held:`), run through this build's
//! xtask on a tracker of a few thousand issues, in the scratch sandbox every black-box test uses
//! (roll-lqjj.42, X17). Two things only a heap can show, neither of which any output does:
//!
//! - no command holds two: a command that hands its transaction (or its state) to the command line
//!   to be kept until the process exits frees it before it publishes or reads a state of its own, so
//!   it peaks where a command that never kept one does;
//! - the libraries never leak: whatever `run_with` made is freed when it returns, a command that
//!   fails as well as one that answers, since a process that does not end with its command (a test
//!   in process, xtask) would otherwise pile one run on another.
//!
//! `cargo xtask bead` runs bead in xtask's process (`run_with`), so these say what a command holds
//! and let go of, not what the process keeps: that is the command line's, tested in the
//! `beadroll` crate (`cli::ending`).
#![cfg(test)]

use beadroll_test_support::{Machine, Run, Sandbox};

/// This build's xtask, which runs bead in its own process, whose allocator counts the heap.
const XTASK: &str = env!("CARGO_BIN_EXE_xtask");

/// How many issues the tracker holds: enough for its projection to outweigh everything else a
/// command holds, so that a second one held beside it is plain.
const ISSUES: usize = 1500;

/// Most a command may still hold when it returns: nothing of the tracker (megabytes), only what
/// the process had before it, bytes.
const LEFT: u64 = 64 * 1024;

/// A peak is the same to within this share when the commands held the same, a whole projection more
/// when one held two.
const SLACK: f64 = 1.05;

/// What a run said of its heap.
#[derive(Clone, Copy, Debug)]
struct Heap {
    /// The most it held at once, in bytes.
    peak: u64,
    /// What it still held when bead returned, in bytes.
    held: u64,
}

/// The number after `label` on a line of `said`.
fn counted(said: &str, label: &str) -> u64 {
    said.lines()
        .find_map(|line| line.strip_prefix(label)?.trim().parse().ok())
        .unwrap_or_else(|| panic!("xtask said no `{label}`: {said}"))
}

/// An export of `count` issues, `ex-1` on, as a tracker's own.
fn export(count: usize) -> String {
    let description = "Some words about the issue, for its text to weigh something. ".repeat(6);
    (1..=count)
        .map(|n| {
            format!(
                "{{\"id\":\"ex-{n}\",\"title\":\"Issue {n}\",\"description\":\"{description}\",\
                 \"status\":\"open\",\"priority\":{},\"issue_type\":\"task\",\
                 \"created_at\":\"2026-01-01T00:00:00Z\",\"updated_at\":\"2026-01-02T00:00:00Z\"}}\n",
                n % 5
            )
        })
        .collect()
}

/// A sandbox whose machine runs `bead` through xtask, and its tracker of `ISSUES` issues, made
/// by importing an export and published; with what the import held.
fn tracker() -> (Sandbox, Machine, Heap) {
    let sandbox = Sandbox::new(XTASK);
    let machine = Machine::new(&sandbox, "a").with_bead(XTASK);
    let file = sandbox.root().join("issues.jsonl");
    std::fs::write(&file, export(ISSUES)).expect("an export");
    let file = file.to_string_lossy().into_owned();
    let (_, heap) = bead(&machine, &["init", "--prefix", "ex", "--import", &file]);
    (sandbox, machine, heap)
}

/// `bead ARGS` through xtask, which must answer, and what it held.
fn bead(machine: &Machine, args: &[&str]) -> (Run, Heap) {
    let (run, heap) = attempt(machine, args);
    assert!(run.status.success(), "bead {args:?} failed: {}", run.stderr);
    (run, heap)
}

/// `bead ARGS` through xtask, whatever it came to, and what it held.
fn attempt(machine: &Machine, args: &[&str]) -> (Run, Heap) {
    let run = machine.run([&["bead"][..], args].concat());
    let heap = Heap {
        peak: counted(&run.stderr, "heap peak:"),
        held: counted(&run.stderr, "heap held:"),
    };
    (run, heap)
}

/// A read, so that the stored projection is current and the command measured next starts from the
/// same store as any other would.
fn settled(machine: &Machine) {
    bead(machine, &["list"]);
}

/// The heap of `bead ARGS` on a settled store.
fn measured(machine: &Machine, args: &[&str]) -> Heap {
    settled(machine);
    bead(machine, args).1
}

/// Every command lets go of what it made by the time `run_with` returns, whatever it was: a read,
/// a write that is not published and one that is, an import, a claim, and a command that fails
/// after it has made its transaction, as a write to an issue that is not there does. A transaction
/// leaked on any of these paths would be the tracker's projection, megabytes held to the end.
#[test]
fn bead_returns_holding_nothing_of_what_it_made() {
    let (_sandbox, machine, import) = tracker();
    assert!(import.held < LEFT, "the import: {import:?}");
    let commands: [(&str, &[&str]); 8] = [
        ("a read", &["list"]),
        (
            "a write, not published",
            &["comments", "add", "ex-1", "one", "--offline"],
        ),
        ("a write, published", &["comments", "add", "ex-1", "two"]),
        ("a claim", &["claim", "ex-2"]),
        ("a close that ends it", &["close", "ex-2"]),
        ("a claim, not published", &["claim", "ex-4", "--offline"]),
        ("a close of another", &["close", "ex-3"]),
        ("a reopen", &["reopen", "ex-3", "--offline"]),
    ];
    for (what, args) in commands {
        let heap = measured(&machine, args);
        assert!(heap.held < LEFT, "{what} held on: {heap:?}");
    }
    for (what, args) in [
        (
            "a write to no issue",
            &["comments", "add", "ex-nothing", "said"][..],
        ),
        ("a claim of no issue", &["claim", "ex-nothing"][..]),
    ] {
        settled(&machine);
        let (run, heap) = attempt(&machine, args);
        assert!(!run.status.success(), "{what} went through: {}", run.stdout);
        assert!(heap.held < LEFT, "{what} held on: {heap:?}");
    }
}

/// No command holds two projections at once. A write that is published at once frees its
/// transaction before the publish makes the state it needs, so it peaks where the same write
/// not published does; a close or delete that ends a claim publishes inside the command and reads
/// the store after, as a claim does, so it frees the transaction it wrote in before either and
/// peaks where the claim did, not a whole projection above it.
#[test]
fn no_command_holds_its_transaction_beside_the_state_it_goes_on_to_read() {
    let (_sandbox, machine, _) = tracker();
    let not_published = measured(&machine, &["comments", "add", "ex-1", "one", "--offline"]);
    let published = measured(&machine, &["comments", "add", "ex-1", "two"]);
    assert!(
        within(published.peak, not_published.peak),
        "a write published at once: {published:?}, not published: {not_published:?}"
    );
    for (op, key) in [("close", "ex-2"), ("delete", "ex-3")] {
        let claim = measured(&machine, &["claim", key]);
        let ending = measured(&machine, &[op, key]);
        assert!(
            within(ending.peak, claim.peak),
            "a {op} that ends a claim: {ending:?}, the claim: {claim:?}"
        );
    }
}

/// Whether `peak` is no more than `other` and a few bytes in a hundred.
fn within(peak: u64, other: u64) -> bool {
    peak as f64 <= other as f64 * SLACK
}
