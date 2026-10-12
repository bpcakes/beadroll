//! V2, verified by the seventh review: a command receipt belongs to the actor whose command left
//! it, so another thread on the same machine never drops one; an attempt still in flight while its
//! actor waits between rounds is fenced or confirmed by another actor's command, and released once;
//! and a pending twin left identical to its confirmed attempt keeps the retry recognised.
//! `V2_InFlightAttemptAcrossActors` and `V2_ConfirmedTwinKeepsTheRetryRecognised` patch bead in
//! process; they are `rust_only`, with the `backoff` and `ownership.after-confirm` failpoints.

use beadroll_store::glob_ending;
use beadroll_test_support::{Json, Machine, Sandbox};

/// `_v2_machine`: machine `name`, its tracker initialised with the prefix `ex` and kept local until
/// its first claim or sync (`--offline`).
fn machine(sandbox: &Sandbox, name: &str) -> Machine {
    let machine = Machine::new(sandbox, name);
    machine.bead(["init", "--prefix", "ex", "--offline"]);
    machine
}

/// `_v2_machine` for a `rust_only` test: machine `name` running the Rust `bead`, its tracker
/// initialised as [`machine`] does, or, with `init` false, the published tracker synced.
fn rust_machine(sandbox: &Sandbox, name: &str, init: bool) -> Machine {
    let machine = crate::rust_only(sandbox, name);
    if init {
        machine.bead(["init", "--prefix", "ex", "--offline"]);
    } else {
        machine.bead(["sync"]);
    }
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

/// The member `key` of `value`; absence fails the test.
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

mod v2_in_flight_attempt_across_actors {
    use std::fs::Permissions;
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::Path;
    use std::time::Duration;

    use beadroll_test_support::git;

    use super::*;
    use crate::hardening::gate::{Gate, beside};

    /// `path` quoted for the shell, as `shlex.quote` quotes it.
    fn quoted(path: &Path) -> String {
        let text = path.to_str().expect("a UTF-8 path");
        format!("'{}'", text.replace('\'', r"'\''"))
    }

    /// `_hook`: `script` as the remote's pre-receive hook.
    fn hook(sandbox: &Sandbox, script: &str) {
        let hook = sandbox.origin().join("hooks").join("pre-receive");
        std::fs::write(&hook, format!("#!/bin/sh\n{script}")).unwrap();
        std::fs::set_permissions(&hook, Permissions::from_mode(0o755)).unwrap();
    }

    /// Waits until `path` exists, polling every 50 ms at most `polls` times.
    fn wait_for(path: &Path, polls: u32) {
        for _ in 0..polls {
            if path.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// `_v2_remote_notes`: the kind of each commit on the remote journal, oldest first ("claim",
    /// "fence", ...).
    fn remote_notes(sandbox: &Sandbox) -> Vec<String> {
        let args = ["log", "--reverse", "--format=%s", "refs/beads/journal"];
        let subjects = git(args, Some(sandbox.origin()));
        let words = subjects.split_whitespace();
        let notes = words.filter(|word| ["claim", "fence", "sync"].contains(word));
        notes.map(str::to_owned).collect()
    }

    /// Actor A's release stalls in a remote hook beyond its one-second timeout. Actor B on the
    /// same machine claims Y while A waits between rounds. With `lands_first`, A's push lands
    /// before B's command. Pause every backoff at a gate: the first waits for B, later waits pass
    /// through it. Returns the pending filenames left by B.
    fn in_flight(lands_first: bool) -> Vec<String> {
        let sandbox = crate::sandbox();
        let n = u8::from(lands_first);
        let m = rust_machine(&sandbox, &format!("v2-flight-{n}"), true);
        // the claim below is the first push the hook sees
        let x = id(m.bead(["create", "X", "--offline"]));
        let y = id(m.bead(["create", "Y", "--offline"]));
        m.cmd(["claim", &x]).thread("actor-a").json();
        m.bead(["sync"]);
        let root = sandbox.root();
        let (gate, started, done) = (root.join("gate"), root.join("started"), root.join("done"));
        std::fs::create_dir(&gate).unwrap();
        let (g, s, d) = (quoted(&gate), quoted(&started), quoted(&done));
        // The first push to arrive (A's release) stalls in the hook until the gate is removed.
        hook(
            &sandbox,
            &format!(
                "if mkdir {s} 2>/dev/null; then\n  while [ -d {g} ]; do sleep 0.05; done\n  \
                 touch {d}; exit 0\nfi\nexit 0\n"
            ),
        );
        let between = Gate::new(&sandbox, "v2-between-rounds");
        let failpoints = format!("backoff@always={}", between.action());
        let (released, seen) = beside(
            || {
                let release = m.cmd(["release", &x, "--json"]).thread("actor-a");
                let release = release.env("BEAD_NET_TIMEOUT", "1");
                release.env("BEAD_FAILPOINTS", &failpoints).run()
            },
            |finished| {
                // A's lock is released and its push is still in flight
                between.meet(finished, || {
                    if lands_first {
                        std::fs::remove_dir(&gate).unwrap();
                        wait_for(&done, 400);
                        std::thread::sleep(Duration::from_millis(300));
                    }
                    let claimed = m.cmd(["claim", &y, "--json"]).thread("actor-b").run();
                    (claimed.returncode(), pending(&m))
                })
            },
        );
        if gate.exists() {
            std::fs::remove_dir(&gate).unwrap();
        }
        // let the stalled receive finish before anything else touches the remote
        wait_for(&done, 200);
        std::thread::sleep(Duration::from_millis(300));
        let (seen, pending_after_b): (Vec<i32>, Vec<String>) =
            seen.map_or_else(Default::default, |(rc, names)| (vec![rc], names));
        assert_eq!(
            (released.returncode(), seen),
            (0, vec![0]),
            "{}",
            released.stderr
        );
        assert_eq!(pending(&m), Vec::<String>::new());
        m.bead(["sync"]);
        let peer = rust_machine(&sandbox, &format!("v2-flight-peer-{n}"), false);
        for machine in [&m, &peer] {
            assert_eq!(machine.show(&x)["status"].as_str(), Some("open"));
            let shown = machine.show(&x);
            assert!(shown.get("claim").is_none(), "{shown:?}");
            assert_eq!(releases(machine, &x), 1);
            let claimed = machine.show(&y);
            assert_eq!(
                member(member(&claimed, "claim"), "thread").as_str(),
                Some("actor-b")
            );
        }
        let expected: &[&str] = if lands_first {
            &["claim", "claim", "claim"]
        } else {
            &["claim", "fence", "claim", "claim"]
        };
        assert_eq!(remote_notes(&sandbox), expected);
        pending_after_b
    }

    /// V2: while A waits outside the lock after a timed-out release, B fences and drops A's
    /// pending attempt and receipt. A then rebuilds and releases exactly once. The backoff gate
    /// in in_flight makes the ordering deterministic; both local and fresh-peer views must agree,
    /// with no pending files.
    #[test]
    fn another_actor_fences_an_in_flight_release_during_the_unlocked_sleep() {
        // B dropped A's attempt and its receipt
        assert_eq!(in_flight(false), Vec::<String>::new());
    }

    /// V2: if A's release lands before B's command, B confirms it while retaining A's receipt.
    /// A's next round recognises that result, with one release and no fence. The backoff gate in
    /// in_flight controls the interleaving.
    #[test]
    fn in_flight_release_lands_before_another_actors_command() {
        let pending_after_b = in_flight(true);
        assert_eq!(pending_after_b.len(), 1, "{pending_after_b:?}");
        assert!(
            pending_after_b[0].ends_with(".command"),
            "{pending_after_b:?}"
        );
    }
}

mod v2_confirmed_twin_keeps_the_retry_recognised {
    use super::*;

    /// V2: after ownership.after-confirm=exit(97), an identical pending twin must retain the
    /// receipt long enough to recognise a retry. Retrying release succeeds without a second
    /// release and leaves no pending files.
    #[test]
    fn identical_pending_twin_confirms_and_the_retry_is_recognised() {
        let sandbox = crate::sandbox();
        let a = rust_machine(&sandbox, "v2-twin", true);
        let x = id(a.bead(["create", "X"]));
        a.bead(["claim", &x]);
        let crashed = a
            .cmd(["release", &x])
            .env("BEAD_FAILPOINTS", "ownership.after-confirm=exit(97)")
            .run();
        assert_eq!(
            crashed.returncode(),
            97,
            "crash after local confirm: {}",
            crashed.stderr
        );
        let left = pending(&a);
        let mut kinds: Vec<&str> = left
            .iter()
            .map(|name| name.rsplit_once('.').map_or("", |(_, kind)| kind))
            .collect();
        kinds.sort_unstable();
        assert_eq!(kinds, ["command", "json"], "{left:?}");
        let name = left.iter().find(|name| name.ends_with(".json")).unwrap();
        let twin = a.store().join("pending").join(name);
        let replicas = std::fs::read_dir(a.store().join("events")).unwrap();
        let confirmed = replicas
            .map(|replica| replica.unwrap().path().join("claims").join(name))
            .find(|path| path.exists())
            .expect("the confirmed attempt");
        assert_eq!(
            std::fs::read(&twin).unwrap(),
            std::fs::read(&confirmed).unwrap()
        );
        let retry = a.run(["release", &x, "--json"]);
        assert_eq!(retry.returncode(), 0, "{}", retry.stderr);
        assert_eq!(pending(&a), Vec::<String>::new());
        assert_eq!(a.show(&x)["status"].as_str(), Some("open"));
        assert_eq!(releases(&a, &x), 1);
    }
}
