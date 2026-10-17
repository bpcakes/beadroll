//! The read after a write (roll-lqjj.51): whatever the write was, offline or published at once, a
//! local edit or an ownership command, the read after it prints what the journal replayed prints,
//! byte for byte.

use beadroll_test_support::Machine;

use super::{projection, tracker};

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

#[test]
fn the_read_after_a_write_prints_what_the_journal_replayed_prints() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "written");
    let keys = tracker(&a);
    for (write, read) in WRITES {
        let (write, read) = (filled(write, &keys), filled(read, &keys));
        let run = a.run(&write);
        assert_eq!(run.returncode(), 0, "{write:?}: {}", run.stderr);
        let after = printed(&a, &read, false);
        assert_eq!(after, printed(&a, &read, true), "{write:?}, then {read:?}");
    }
}
