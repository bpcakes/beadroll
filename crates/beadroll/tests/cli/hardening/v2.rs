//! V2, verified by the seventh review: a command receipt belongs to the actor whose command left
//! it, so another thread on the same machine never drops one. `V2_InFlightAttemptAcrossActors`
//! and `V2_ConfirmedTwinKeepsTheRetryRecognised` need the `backoff` and `ownership.after-confirm`
//! failpoints, and are ported with them.

use beadroll_store::glob_ending;
use beadroll_test_support::{Json, Machine, Sandbox};

/// `_v2_machine`: machine `name`, its tracker initialised with the prefix `ex` and kept local until
/// its first claim or sync (`--offline`).
fn machine(sandbox: &Sandbox, name: &str) -> Machine {
    let machine = Machine::new(sandbox, name);
    machine.bead(["init", "--prefix", "ex", "--offline"]);
    machine
}

/// `_v2_pending`: the names in `machine`'s `pending/`, sorted; none when there is no such folder.
fn pending(machine: &Machine) -> Vec<String> {
    let folder = machine.store().join("pending");
    if !folder.is_dir() {
        return Vec::new();
    }
    let entries = std::fs::read_dir(&folder).unwrap();
    let mut names: Vec<String> = entries
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// `_v2_releases`: how many of `key`'s events are releases, as `bead log KEY --limit 0` lists them.
fn releases(machine: &Machine, key: &str) -> usize {
    let log = machine.bead(["log", key, "--limit", "0"]).expect("a log");
    let events = log.as_array().expect("bead log prints a list");
    events
        .iter()
        .filter(|event| member(event, "op").as_str() == Some("release"))
        .count()
}

/// The id of the issue a `--json` result is: `result["id"]`.
fn id(result: Option<Json>) -> String {
    let result = result.expect("bead printed a result");
    result["id"].as_str().expect("an id").to_owned()
}

/// `value[key]` as Python indexes a dict: the member, or the test fails.
fn member<'v>(value: &'v Json, key: &str) -> &'v Json {
    value
        .get(key)
        .unwrap_or_else(|| panic!("no {key:?} in {value:?}"))
}

mod v2_receipts_are_per_actor {
    use super::*;

    /// Verified (V2): receipts are per actor. Another thread on the same machine whose `claim Z`
    /// crashes after its build (before the push, rc 98) and whose `claim Y` then succeeds (rc 0)
    /// leaves A's settled receipt for its lost-ack `release X` in place, and A's retry is
    /// recognised. The review measured 1 receipt left after B's rc-98 and rc-0 claims and A's retry
    /// `release X` rc 0 with X open (1/1 test run plus 1 repro run).
    #[test]
    fn other_actor_on_the_same_machine_never_drops_a_receipt() {
        let sandbox = crate::sandbox();
        let a = machine(&sandbox, "v2-actors");
        let x = id(a.bead(["create", "X"]));
        let y = id(a.bead(["create", "Y"]));
        let z = id(a.bead(["create", "Z"]));
        a.bead(["claim", &x]);
        a.bead(["sync"]);
        let lost = a
            .cmd(["release", &x, "--json"])
            .env("BEAD_TEST_CRASH_AFTER_PUSH", "1")
            .run();
        assert_eq!(lost.returncode(), 99, "{}", lost.stderr);
        let other = a
            .cmd(["claim", &z, "--json"])
            .thread("other")
            .env("BEAD_TEST_CRASH_BEFORE_PUSH", "1")
            .run();
        assert_eq!(other.returncode(), 98, "{}", other.stderr);
        let landed = a.cmd(["claim", &y, "--json"]).thread("other").run();
        assert_eq!(landed.returncode(), 0, "{}", landed.stderr);
        let receipts = glob_ending(&a.store().join("pending"), ".command");
        assert_eq!(receipts.len(), 1, "{:?}", pending(&a));
        let retry = a.run(["release", &x, "--json"]);
        assert_eq!(retry.returncode(), 0, "{}", retry.stderr);
        assert_eq!(a.show(&x)["status"].as_str(), Some("open"));
        let shown = a.show(&x);
        assert!(shown.get("claim").is_none(), "{shown:?}");
        assert_eq!(releases(&a, &x), 1);
        let claimed = a.show(&y);
        assert_eq!(
            member(member(&claimed, "claim"), "thread").as_str(),
            Some("other")
        );
    }
}
