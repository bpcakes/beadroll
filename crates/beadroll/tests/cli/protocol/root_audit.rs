//! `RootAudit`: the first independent audit's ownership checks, against a real coordination
//! remote (`docs/reviews/2026-10-06-1-audit.md`, A09). Claims, takeovers and releases raced from
//! several machines end where the operations the remote accepted lead, with one confirmed owner at
//! most and generations that only grow; a forced close or a tombstone ends ownership; a
//! provisional claim can be released, or replaced by its thread's online claim; and a status edit
//! obeys the ownership rules `close` does. The clock-skew test needs the `clock` failpoint and is
//! ported with it.

use std::sync::Barrier;

use beadroll_test_support::{Json, Machine, Run, Sandbox, code, parse_json};

/// `setUp`: machine `root-a` has initialised tracker `root`, created the issue every test works
/// on and synced. Returns the machine and the issue's id.
fn set_up(sandbox: &Sandbox) -> (Machine, String) {
    let a = Machine::new(sandbox, "root-a");
    a.bead(["init", "--prefix", "root"]);
    let created = a.bead(["create", "ownership check"]).expect("the issue");
    let key = created["id"].as_str().expect("an id").to_owned();
    a.bead(["sync"]);
    (a, key)
}

/// `self.machine(name)`: another machine, synced.
fn machine(sandbox: &Sandbox, name: &str) -> Machine {
    let machine = Machine::new(sandbox, name);
    machine.bead(["sync"]);
    machine
}

/// `self.settle(machines)`: every machine syncs twice, each sync reporting no forked journal and no
/// event waiting for its causes; then every machine shows `key` as the first does, and that issue
/// is returned.
fn settle(machines: &[&Machine], key: &str) -> Json {
    for machine in machines.iter().chain(machines) {
        let report = machine.bead(["sync"]).expect("a sync report");
        assert_eq!(
            report["forked_journals"],
            Json::Array(Vec::new()),
            "{report:?}"
        );
        assert_eq!(report["waiting_events"].as_i64(), Some(0), "{report:?}");
    }
    let expected = machines[0].show(key);
    for machine in &machines[1..] {
        assert_eq!(machine.show(key), expected, "{} disagrees", machine.name());
    }
    expected
}

/// `value[key]` as Python indexes a dict: the member, or the test fails.
fn member<'v>(value: &'v Json, key: &str) -> &'v Json {
    value
        .get(key)
        .unwrap_or_else(|| panic!("no {key:?} in {value:?}"))
}

/// The claim a successful `bead ... --json` printed: `json.loads(run.stdout)[0]["claim"]`.
fn claim_of(run: &Run) -> Json {
    let printed = parse_json(&run.stdout)
        .unwrap_or_else(|error| panic!("bead printed no JSON ({error}): {}", run.stdout));
    member(&printed[0], "claim").clone()
}

/// `bead ARGS` from each `(machine, ARGS)` at the same instant, a thread each, released together
/// once every thread has started: the runs in the order given, as the futures of a
/// `ThreadPoolExecutor` with a worker for each give them.
fn race(runs: &[(&Machine, &[&str])]) -> Vec<Run> {
    let start = Barrier::new(runs.len());
    std::thread::scope(|scope| {
        let threads: Vec<_> = runs
            .iter()
            .map(|&(machine, args)| {
                let start = &start;
                scope.spawn(move || {
                    start.wait();
                    machine.run(args)
                })
            })
            .collect();
        threads
            .into_iter()
            .map(|thread| thread.join().expect("a racing bead's thread"))
            .collect()
    })
}

/// Six machines claim one issue at the same instant: one claim wins, the other five are refused
/// with `ALREADY_CLAIMED`, and once they have all settled every machine shows the winner's claim.
#[test]
fn six_simultaneous_claims_have_one_winner() {
    let sandbox = crate::sandbox();
    let (a, key) = set_up(&sandbox);
    let others: Vec<Machine> = (0..5)
        .map(|n| machine(&sandbox, &format!("root-{n}")))
        .collect();
    let machines: Vec<&Machine> = std::iter::once(&a).chain(&others).collect();
    let claim = ["claim", key.as_str(), "--json"];
    let runs: Vec<(&Machine, &[&str])> = machines.iter().map(|&m| (m, &claim[..])).collect();
    let results = race(&runs);
    let mut codes: Vec<i32> = results.iter().map(Run::returncode).collect();
    codes.sort_unstable();
    assert_eq!(codes, [0, 1, 1, 1, 1, 1], "{results:?}");
    for result in &results {
        if result.returncode() != 0 {
            assert_eq!(code(result), "ALREADY_CLAIMED");
        }
    }
    let won = results
        .iter()
        .find(|r| r.returncode() == 0)
        .expect("a winner");
    let winner = claim_of(won);
    assert_eq!(member(&settle(&machines, &key), "claim"), &winner);
}

/// The owner's release races another machine's claim: the release succeeds, and the issue settles
/// as the order the remote accepted them in makes it. Either the claim came first and was refused
/// with `ALREADY_CLAIMED`, and nobody holds the issue, or it came after the release and holds it,
/// one generation after the released claim.
#[test]
fn claim_racing_release_matches_accepted_outcome() {
    let sandbox = crate::sandbox();
    let (a, key) = set_up(&sandbox);
    let b = machine(&sandbox, "root-b");
    let claimed = a.bead(["claim", &key]).expect("the claimed issue");
    let first = member(&claimed[0], "claim");
    let generation = first["generation"].as_i64().expect("a generation");
    let raced = race(&[
        (&a, &["release", &key, "--json"]),
        (&b, &["claim", &key, "--json"]),
    ]);
    let (released, claim) = (&raced[0], &raced[1]);
    assert_eq!(released.returncode(), 0, "{}", released.stderr);
    let issue = settle(&[&a, &b], &key);
    if claim.returncode() != 0 {
        assert_eq!(code(claim), "ALREADY_CLAIMED");
        assert!(issue.get("claim").is_none(), "{issue:?}");
    } else {
        let held = member(&issue, "claim");
        assert_eq!(member(held, "id"), member(&claim_of(claim), "id"));
        assert_eq!(held["generation"].as_i64(), Some(generation + 1));
    }
}

/// Two machines take over one claimed issue at the same instant: the takeovers are serialised and
/// both succeed, at generations 2 and 3, and every machine settles on the generation 3 claim.
#[test]
fn two_takeovers_are_serial_and_generation_increases() {
    let sandbox = crate::sandbox();
    let (a, key) = set_up(&sandbox);
    let (b, c) = (machine(&sandbox, "root-b"), machine(&sandbox, "root-c"));
    a.bead(["claim", &key]);
    let takeover = ["takeover", key.as_str(), "--reason", "synthetic", "--json"];
    let results = race(&[(&b, &takeover), (&c, &takeover)]);
    let codes: Vec<i32> = results.iter().map(Run::returncode).collect();
    assert_eq!(codes, [0, 0], "{results:?}");
    let claims: Vec<Json> = results.iter().map(claim_of).collect();
    let mut generations: Vec<i64> = claims
        .iter()
        .map(|claim| claim["generation"].as_i64().expect("a generation"))
        .collect();
    generations.sort_unstable();
    assert_eq!(generations, [2, 3]);
    let last = claims
        .iter()
        .find(|claim| claim["generation"].as_i64() == Some(3));
    let last = last.expect("the generation 3 claim");
    assert_eq!(member(&settle(&[&a, &b, &c], &key), "claim"), last);
}

/// The owner's release races another machine's takeover: the takeover succeeds, the release either
/// lands before it or is refused with `NOT_OWNER`, and the issue settles with the new owner's
/// claim, generation 2.
#[test]
fn takeover_racing_release_ends_at_new_owner() {
    let sandbox = crate::sandbox();
    let (a, key) = set_up(&sandbox);
    let b = machine(&sandbox, "root-b");
    a.bead(["claim", &key]);
    let raced = race(&[
        (&a, &["release", &key, "--json"]),
        (&b, &["takeover", &key, "--reason", "synthetic", "--json"]),
    ]);
    let (released, taken) = (&raced[0], &raced[1]);
    assert_eq!(taken.returncode(), 0, "{}", taken.stderr);
    if released.returncode() != 0 {
        assert_eq!(code(released), "NOT_OWNER");
    }
    let settled = settle(&[&a, &b], &key);
    let held = member(&settled, "claim");
    assert_eq!(member(held, "id"), member(&claim_of(taken), "id"));
    assert_eq!(held["generation"].as_i64(), Some(2));
}

/// Another machine's forced close ends the owner's claim: the closed issue cannot be claimed
/// (`VALIDATION`) and has no claim; once reopened, its next claim is generation 2; and once it is
/// deleted, its tombstone cannot be claimed either.
#[test]
fn forced_close_tombstone_and_reopen_generation() {
    let sandbox = crate::sandbox();
    let (a, key) = set_up(&sandbox);
    let b = machine(&sandbox, "root-b");
    a.bead(["claim", &key]);
    b.bead(["sync"]);
    b.bead(["close", &key, "--force"]);
    assert_eq!(code(&a.run(["claim", &key, "--json"])), "VALIDATION");
    let issue = a.show(&key);
    assert!(issue.get("claim").is_none(), "{issue:?}");
    a.bead(["reopen", &key]);
    let claimed = a.bead(["claim", &key]).expect("the claimed issue");
    assert_eq!(claimed[0]["claim"]["generation"].as_i64(), Some(2));
    a.bead(["release", &key]);
    a.bead(["delete", &key]);
    assert_eq!(code(&a.run(["claim", &key, "--json"])), "VALIDATION");
}

/// A provisional claim, made offline, can be released offline, which leaves the issue open with no
/// provisional claim; and a thread's online claim over its own provisional one replaces it: the
/// confirmed claim is generation 1, and no provisional claim is left.
#[test]
fn provisional_release_and_same_thread_online_upgrade() {
    let sandbox = crate::sandbox();
    let (a, key) = set_up(&sandbox);
    a.bead(["claim", &key, "--offline"]);
    a.bead(["release", &key, "--offline"]);
    let issue = a.show(&key);
    assert_eq!(issue["status"].as_str(), Some("open"));
    assert!(issue.get("provisional_claims").is_none(), "{issue:?}");
    a.bead(["claim", &key, "--offline"]);
    let claimed = a.bead(["claim", &key]).expect("the claimed issue");
    let issue = &claimed[0];
    assert_eq!(issue["claim"]["generation"].as_i64(), Some(1));
    assert!(issue.get("provisional_claims").is_none(), "{issue:?}");
}

/// A09: a thread that does not hold the claim can neither close the issue (`CLAIMED_BY_OTHER`) nor
/// get around that guard with `update --status closed`, which would leave the claim live.
#[test]
fn nonowner_cannot_bypass_close_guard_with_status_update() {
    let sandbox = crate::sandbox();
    let (a, key) = set_up(&sandbox);
    a.cmd(["claim", &key]).thread("owner-thread").json();
    let close = a
        .cmd(["close", &key, "--json"])
        .thread("other-thread")
        .run();
    assert_eq!(code(&close), "CLAIMED_BY_OTHER");
    let status = ["update", &key, "--status", "closed", "--json"];
    let changed = a.cmd(status).thread("other-thread").run();
    assert_ne!(
        changed.returncode(),
        0,
        "update --status closed bypassed close ownership check and left claim live"
    );
}

/// A09: after a thread that does not hold the claim sets the issue's status to open, `ready` still
/// does not offer it, since it has a confirmed owner.
#[test]
fn claimed_issue_is_not_ready_after_nonowner_status_update() {
    let sandbox = crate::sandbox();
    let (a, key) = set_up(&sandbox);
    a.cmd(["claim", &key]).thread("owner-thread").json();
    let status = ["update", &key, "--status", "open", "--json"];
    a.cmd(status).thread("other-thread").run();
    assert!(
        !a.ready().contains(&key),
        "ready offers an issue that still has a confirmed owner"
    );
}
