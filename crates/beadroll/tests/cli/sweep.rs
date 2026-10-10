//! `tests/test_sweep.py`, ported (`docs/RUST-PORT.md` §4.3, §4.8): the ownership protocol,
//! enumerated (seventh independent review, 2026-10-07). Three machines, nine operations at seven
//! fault boundaries, every ordered pair of four follow-ups; after each step at most one confirmed
//! owner, generations monotonic, identical exports after settling, and every command's answer
//! consistent with the remote. The default is a twelve-schedule slice touching every operation and
//! boundary, including retries of an owner's close/delete interrupted before publishing
//! ([`SLICE`]). `BEAD_SWEEP=full` runs all 660; `BEAD_SWEEP_GC=1` and `BEAD_SWEEP_CONCURRENT=1`
//! select the variants. Schedules run in parallel, each in its own sandbox.
//!
//! The Python sweep reached its boundaries inside one bead process through a `runpy` wrapper that
//! patched bead's module globals. This port sets failpoints instead (`BEAD_FAILPOINTS`, §4.5),
//! which only the Rust binary has, so it is `rust_only`: its machines run the `bead` Cargo built
//! whatever `BEAD_UNDER_TEST` says ([`crate::rust_only`]). [`fault_env`] gives each boundary's
//! failpoint.

use std::collections::BTreeMap;

mod call;
mod judge;
mod run;
#[allow(
    clippy::module_inception,
    reason = "the inventory's rule names the port of Sweep.test_sweep sweep::sweep::sweep"
)]
mod sweep;

/// The threads: two actors on m1, one each on m2 and m3.
const A1: &str = "m1-a1";
const A2: &str = "m1-a2";
const B1: &str = "m2-b1";
const C1: &str = "m3-c1";

/// What an operation needs before it runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pre {
    /// Nothing: X is free.
    Free,
    /// A1 on m1 holds X, and every machine has seen it.
    A1Holds,
    /// B1 on m2 holds X, and every machine has seen it.
    B1Holds,
    /// Local work on m1 to publish, and another machine's attempt (B1's claim of Y) to copy in.
    Sync,
}

/// An operation of the sweep: what it needs, and its arguments. Every one runs as A1 on m1.
struct Op {
    name: &'static str,
    pre: Pre,
    argv: fn(&Keys) -> Vec<String>,
}

/// The words of a command line.
fn words(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| (*word).to_owned()).collect()
}

/// The nine operations, in the order Python's `OPS` lists them.
const OPS: [Op; 9] = [
    Op {
        name: "claim",
        pre: Pre::Free,
        argv: |k| words(&["claim", k.of('X')]),
    },
    Op {
        name: "release",
        pre: Pre::A1Holds,
        argv: |k| words(&["release", k.of('X')]),
    },
    Op {
        name: "takeover",
        pre: Pre::B1Holds,
        argv: |k| words(&["takeover", k.of('X'), "--reason", "sweep"]),
    },
    Op {
        name: "close",
        pre: Pre::A1Holds,
        argv: |k| words(&["close", k.of('X'), "--reason", "done"]),
    },
    Op {
        name: "delete",
        pre: Pre::A1Holds,
        argv: |k| words(&["delete", k.of('X'), "--reason", "gone"]),
    },
    Op {
        name: "update-edits",
        pre: Pre::Free,
        argv: |k| {
            let x = k.of('X');
            words(&[
                "update",
                x,
                "--claim",
                "--title",
                "edited",
                "--add-label",
                "swept",
            ])
        },
    },
    Op {
        name: "update-parent",
        pre: Pre::Free,
        argv: |k| words(&["update", k.of('X'), "--claim", "--parent", k.of('P')]),
    },
    Op {
        name: "update-closed",
        pre: Pre::Free,
        argv: |k| words(&["update", k.of('X'), "--claim", "--status", "closed"]),
    },
    Op {
        name: "sync",
        pre: Pre::Sync,
        argv: |_| words(&["sync"]),
    },
];

/// The operations that run the ownership protocol, coordinated with the remote.
const COORDINATED: [&str; 6] = [
    "claim",
    "release",
    "takeover",
    "update-edits",
    "update-parent",
    "update-closed",
];

/// The seven fault boundaries.
const FAULTS: [&str; 7] = [
    "crash-before-push",
    "crash-after-push",
    "timeout-fenced",
    "timeout-landed",
    "kill-confirm",
    "kill-write-pending",
    "kill-write-confirm",
];

/// Every ordered pair of two different follow-ups, in the order `itertools.permutations("RDOS",
/// 2)` gives them: R a retry of the same command by A1 on m1, D `claim Z` by A1 on m1, O `claim X`
/// by A2 on m1, S `sync` on m2.
const PAIRS: [&str; 12] = [
    "RD", "RO", "RS", "DR", "DO", "DS", "OR", "OD", "OS", "SR", "SD", "SO",
];

/// Which boundaries an operation passes through. close, delete and sync publish through
/// `synchronise`, which has no crash hook: the push boundary is reached with a failpoint on the
/// push instead. They never write or confirm an attempt of their own; a sync copies other
/// machines' attempts, so a kill during that write applies to it.
fn applicable(op: &str, fault: &str) -> bool {
    match fault {
        "kill-confirm" | "kill-write-pending" => COORDINATED.contains(&op),
        "kill-write-confirm" => COORDINATED.contains(&op) || op == "sync",
        _ => true,
    }
}

/// One schedule: an operation, the boundary it is stopped at, and the two follow-ups after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Schedule {
    op: &'static str,
    fault: &'static str,
    pair: &'static str,
}

/// The 660 schedules, in Python's order.
fn schedules() -> Vec<Schedule> {
    let mut all = Vec::new();
    for op in &OPS {
        for fault in FAULTS
            .into_iter()
            .filter(|fault| applicable(op.name, fault))
        {
            all.extend(PAIRS.map(|pair| Schedule {
                op: op.name,
                fault,
                pair,
            }));
        }
    }
    all
}

/// A schedule, as the slice lists it.
const fn schedule(op: &'static str, fault: &'static str, pair: &'static str) -> Schedule {
    Schedule { op, fault, pair }
}

/// The slice the fast suite can afford: every operation and boundary, with pairs that mix a retry
/// with another actor or machine, including the owner's interrupted close/delete (G12).
const SLICE: [Schedule; 12] = [
    schedule("claim", "crash-after-push", "RO"),
    schedule("release", "crash-after-push", "OR"),
    schedule("takeover", "timeout-landed", "SR"),
    schedule("close", "crash-before-push", "DR"),
    schedule("delete", "crash-after-push", "SO"),
    schedule("update-edits", "kill-confirm", "RO"),
    schedule("update-parent", "kill-write-pending", "DR"),
    schedule("update-closed", "timeout-fenced", "OR"),
    schedule("sync", "kill-write-confirm", "SR"),
    schedule("claim", "timeout-landed", "OS"),
    schedule("close", "crash-before-push", "RS"),
    schedule("delete", "crash-before-push", "OR"),
];

/// How every schedule of a sweep runs: `gc`, the remote collects garbage after every push it
/// accepts (`BEAD_SWEEP_GC=1`); `concurrent`, the two follow-ups run at once
/// (`BEAD_SWEEP_CONCURRENT=1`).
#[derive(Clone, Copy, Debug)]
struct Variant {
    gc: bool,
    concurrent: bool,
}

/// The issues of a schedule's tracker, by letter: Q, X's parent at first; the target X; Y, Z and
/// P, the parent `update-parent` gives X.
#[derive(Debug, Default)]
struct Keys(BTreeMap<char, String>);

impl Keys {
    /// The key of the issue `letter`.
    fn of(&self, letter: char) -> &str {
        self.0
            .get(&letter)
            .unwrap_or_else(|| panic!("no issue {letter} in the tracker"))
    }
}

/// What injects `fault` into the run of `op`: a variable and its value. Each boundary the Python
/// wrapper (or a crash hook) stopped bead at is the failpoint §4.5 gives, fired at the hit the
/// wrapper fired at:
///
/// - crash-before-push: a coordinated operation stops (exit 98) once its receipt and attempt are
///   written, before they are published: `ownership.before-publish=exit(98)`, which is what
///   `BEAD_TEST_CRASH_BEFORE_PUSH`, the hook Python's sweep set, turns on. close, delete and sync
///   stop before their first `git push`: `push.before=exit(98)`.
/// - crash-after-push: exit 99 once the remote accepted, the acknowledgement lost:
///   `ownership.after-publish-ok=exit(99)` (`BEAD_TEST_CRASH_AFTER_PUSH`), or after the first push
///   the remote accepted, `push.after-ok=exit(99)`.
/// - kill-confirm: SIGKILL once the accepted attempt counts here, before its pending twin is
///   dropped: `ownership.after-confirm=sigkill`.
/// - kill-write-pending: SIGKILL while the attempt `pending/<id>.json` is written, its receipt
///   `pending/<id>.command` already in place: `atomic-write:pending/@nth(2)=sigkill`, since the
///   receipt is the first write under `pending/`, which the wrapper let through.
/// - kill-write-confirm: SIGKILL during the first write of an attempt into `claims/`, the
///   confirmation of the operation's own attempt or, for a sync, the copy of another machine's:
///   `atomic-write:claims/=sigkill`.
/// - timeout-fenced, timeout-landed: no failpoint; the remote's pre-receive hook stalls the first
///   push past `BEAD_NET_TIMEOUT` (`Run::fault_args` installs it).
///
/// The wrapper killed a write with half the bytes under the temporary name; `atomic-write` fires
/// with all of them there, fsynced, before the rename. Either way the target is not written and
/// the temporary is left behind.
fn fault_env(op: &str, fault: &str) -> (&'static str, &'static str) {
    let coordinated = COORDINATED.contains(&op);
    let spec = match (fault, coordinated) {
        ("crash-before-push", true) => "ownership.before-publish=exit(98)",
        ("crash-before-push", false) => "push.before=exit(98)",
        ("crash-after-push", true) => "ownership.after-publish-ok=exit(99)",
        ("crash-after-push", false) => "push.after-ok=exit(99)",
        ("kill-confirm", _) => "ownership.after-confirm=sigkill",
        ("kill-write-pending", _) => "atomic-write:pending/@nth(2)=sigkill",
        ("kill-write-confirm", _) => "atomic-write:claims/=sigkill",
        _ => return ("BEAD_NET_TIMEOUT", NET_TIMEOUT),
    };
    ("BEAD_FAILPOINTS", spec)
}

/// The first push stalls past the client's timeout, then lets receive-pack try its update; every
/// later push (the fence) goes straight through, so the fence lands first and the stalled one
/// loses. `{root}` is the sandbox, `{stall}` the seconds of the stall.
const STALL_FENCED: &str = "#!/bin/sh
# The first push stalls past the client's timeout, then lets receive-pack try its update; every
# later push (the fence) goes straight through, so the fence lands first and the stalled one loses.
if mkdir {root}/hook-first 2>/dev/null; then
  touch {root}/hook-started; sleep {stall}; touch {root}/hook-finished
fi
exit 0
";

/// The first push stalls past the client's timeout and then lands; every later push (the fence)
/// waits behind it, so the first one wins and the fence's ref update fails.
const STALL_LANDED: &str = "#!/bin/sh
# The first push stalls past the client's timeout and then lands; every later push (the fence)
# waits behind it, so the first one wins and the fence's ref update fails.
if mkdir {root}/hook-first 2>/dev/null; then
  touch {root}/hook-started; sleep {stall}; touch {root}/hook-finished; exit 0
fi
n=0; while [ ! -f {root}/hook-finished ] && [ $n -lt 400 ]; do sleep 0.05; n=$((n+1)); done
sleep 0.3
exit 0
";

/// The client's timeout during a timeout fault, and how long the first push stalls.
const NET_TIMEOUT: &str = "1.0";
const STALL: &str = "3.5";

/// The codes of an answer that says the outcome is not known: excused, as exits 98, 99 and -9 are.
const UNCERTAIN: [&str; 3] = ["UNRESOLVED", "REMOTE_UNREACHABLE", "SYNC_CONFLICT"];
