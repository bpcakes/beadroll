//! Enumerates the ownership protocol across three machines, nine operations, seven fault
//! boundaries and ordered pairs of four follow-ups. Checks single ownership, monotonic
//! generations, convergence after settling and agreement between command results and remote
//! state. Each schedule must prove its fault fired ([`fault_fired`]).
//!
//! The default thirteen-schedule slice covers every operation and boundary, interrupted owner
//! close/delete retries and a sync stopped at push ([`SLICE`]). `BEAD_SWEEP=full` runs all 660;
//! `BEAD_SWEEP_GC=1` and `BEAD_SWEEP_CONCURRENT=1` select variants. Schedules run in parallel in
//! separate sandboxes. Build-specific failpoints require [`crate::rust_only`]; [`fault_env`] maps
//! boundaries to their controls.

use std::collections::BTreeMap;

use call::Rc;

mod call;
mod judge;
mod run;
#[allow(
    clippy::module_inception,
    reason = "Keep sweep::sweep::sweep stable for test filters and findings references"
)]
mod sweep;
mod sweep_failure_reporting;

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
    /// Local work on m1 to publish, recorded offline so that the sync has it to push, and another
    /// machine's attempt (B1's claim of Y) to copy in.
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

/// The nine operations in schedule enumeration order.
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

/// The 660 schedules in deterministic enumeration order.
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
/// with another actor or machine, including the owner's interrupted close/delete (G12) and a sync
/// whose push lands unacknowledged (roll-dih8.10).
const SLICE: [Schedule; 13] = [
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
    schedule("sync", "crash-after-push", "RS"),
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

/// The environment setting that injects `fault` into `op`. Coordinated operations stop before
/// publication at `ownership.before-publish=exit(98)`; close, delete and sync stop at
/// `push.before=exit(98)`. Lost acknowledgements use `ownership.after-publish-ok=exit(99)` or
/// `push.after-ok=exit(99)`.
///
/// Confirmation stops at `ownership.after-confirm=sigkill`, after acceptance counts but before
/// pending-twin removal. Pending writes stop at `atomic-write:pending/@nth(2)=sigkill`: the
/// receipt is the first write and the attempt the second. Confirmed writes stop at
/// `atomic-write:claims/=sigkill`. Atomic-write failures occur after the temporary file is
/// written and fsynced, before rename, leaving the target untouched and temporary file behind.
///
/// Timeout schedules use no failpoint: a pre-receive hook stalls the first push beyond
/// BEAD_NET_TIMEOUT (`Run::fault_args`).
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

/// Whether the run of the operation shows that `fault` fired, ending it at its boundary: the exit
/// its failpoint gives (98 before the push, 99 once the remote accepted it), a SIGKILL (-9), or,
/// for a timeout, the remote's stall hook started (`stalled`) by the operation's push, which the
/// client then waited on past its timeout. Any other end means the operation never got there.
fn fault_fired(fault: &str, rc: Rc, stalled: bool) -> bool {
    match fault {
        "crash-before-push" => rc.is(98),
        "crash-after-push" => rc.is(99),
        "kill-confirm" | "kill-write-pending" | "kill-write-confirm" => rc.is(-9),
        _ => stalled,
    }
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
