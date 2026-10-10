//! G08 (`docs/FINDINGS.md`): stale receipts were dropped right after `build`, before the push; a
//! failure after that lost them, and the retry rebuilt an accepted command. The tests stop commands
//! on the crash hooks both binaries implement: `BEAD_TEST_CRASH_AFTER_PUSH` (exit status 99 once
//! the remote has taken the push) and `BEAD_TEST_CRASH_BEFORE_PUSH` (98, before the push).

use beadroll_test_support::{Machine, Sandbox};

/// `_g08_machine`: a machine `name` that initialised the tracker (prefix `ex`), or, with `init`
/// false, synced the one already published.
fn machine(sandbox: &Sandbox, name: &str, init: bool) -> Machine {
    let machine = Machine::new(sandbox, name);
    if init {
        machine.bead(["init", "--prefix", "ex"]);
    } else {
        machine.bead(["sync"]);
    }
    machine
}

/// coordinated drops the actor's settled receipts as soon as a later command's build succeeds
/// (bead 1910), before its push. When that later command then fails, the retry of the earlier,
/// accepted command is no longer recognised, although REVIEWING section 2 says a receipt survives
/// a failed later command by the same actor.
mod g08_stale_receipts_dropped_before_the_command_goes_through {
    use std::fs::Permissions;
    use std::os::unix::fs::PermissionsExt as _;
    use std::time::Duration;

    use beadroll_test_support::{Invocation, Json, Machine, code};

    use super::machine;

    /// The id of the issue a `bead create --json` made: `result["id"]`.
    fn id(created: Option<Json>) -> String {
        let created = created.expect("bead printed the issue it made");
        created["id"].as_str().expect("an id").to_owned()
    }

    /// `bead ARGS` from `machine`, stopped by the crash hook right after its push was accepted.
    fn crashing_after_push<'m>(machine: &'m Machine, args: &[&str]) -> Invocation<'m> {
        machine.cmd(args).env("BEAD_TEST_CRASH_AFTER_PUSH", "1")
    }

    /// `bead ARGS` from `machine`, stopped by the crash hook just before its push.
    fn crashing_before_push<'m>(machine: &'m Machine, args: &[&str]) -> Invocation<'m> {
        machine.cmd(args).env("BEAD_TEST_CRASH_BEFORE_PUSH", "1")
    }

    /// G08: a later `claim Y` that crashed before its push did not go through; the retry of the
    /// accepted claiming close is still recognised (not VALIDATION "is closed").
    #[test]
    fn crash_before_push_of_a_later_command_keeps_the_accepted_close() {
        let sandbox = crate::sandbox();
        let a = machine(&sandbox, "g08-crash", true);
        let x = id(a.bead(["create", "X"]));
        let y = id(a.bead(["create", "Y"]));
        a.bead(["sync"]);
        let close = ["update", &x, "--claim", "--status", "closed", "--json"];
        let first = crashing_after_push(&a, &close).run();
        assert_eq!(first.returncode(), 99, "{}", first.stderr);
        let later = crashing_before_push(&a, &["claim", &y, "--json"]).run();
        // the later command failed: it never reached the remote
        assert_eq!(later.returncode(), 98, "{}", later.stderr);
        let retry = a.run(close);
        assert_eq!(
            retry.returncode(),
            0,
            "retry of the accepted close not recognised: {}",
            retry.stderr
        );
    }

    /// G08: a later `claim Y` that ended UNRESOLVED (and never landed) did not go through; the
    /// retry of the accepted release is still recognised (not NOT_OWNER).
    #[test]
    fn unresolved_later_command_keeps_the_accepted_release() {
        let sandbox = crate::sandbox();
        let a = machine(&sandbox, "g08-unresolved", true);
        let x = id(a.bead(["create", "X"]));
        let y = id(a.bead(["create", "Y"]));
        a.bead(["claim", &x]);
        a.bead(["sync"]);
        let released = crashing_after_push(&a, &["release", &x, "--json"]).run();
        assert_eq!(released.returncode(), 99, "{}", released.stderr);
        let gate = sandbox.root().join("gate");
        std::fs::create_dir(&gate).unwrap();
        let hook = sandbox.origin().join("hooks").join("pre-receive");
        let script = format!(
            "#!/bin/sh\nwhile [ -d '{}' ]; do sleep 0.05; done\nexit 1\n",
            gate.display()
        );
        std::fs::write(&hook, script).unwrap();
        std::fs::set_permissions(&hook, Permissions::from_mode(0o755)).unwrap();
        let later = a
            .cmd(["claim", &y, "--json"])
            .env("BEAD_NET_TIMEOUT", "1")
            .run();
        std::fs::remove_dir(&gate).unwrap();
        // the held pushes are refused once the gate is gone
        std::thread::sleep(Duration::from_secs(1));
        std::fs::remove_file(&hook).unwrap();
        assert_eq!(later.returncode(), 1, "{}", later.stderr);
        assert_eq!(code(&later), "UNRESOLVED");
        let retry = a.run(["release", &x, "--json"]);
        assert_eq!(
            retry.returncode(),
            0,
            "retry of the accepted release not recognised: {}",
            retry.stderr
        );
    }

    /// G08: after a later command of the same actor failed before its push, the retry of an
    /// accepted `update --claim --title` must not apply the title a second time over another
    /// machine's newer one.
    #[test]
    fn failed_later_command_does_not_make_the_retry_reapply_edits() {
        let sandbox = crate::sandbox();
        let a = machine(&sandbox, "g08-overwrite", true);
        let x = id(a.bead(["create", "X"]));
        let y = id(a.bead(["create", "Y"]));
        a.bead(["sync"]);
        let b = machine(&sandbox, "g08-overwrite-b", false);
        let retitle = ["update", &x, "--claim", "--title", "from-A", "--json"];
        let crashed = crashing_after_push(&a, &retitle).run();
        assert_eq!(crashed.returncode(), 99, "{}", crashed.stderr);
        b.bead(["sync"]);
        b.bead(["update", &x, "--title", "from-B"]);
        b.bead(["sync"]);
        let later = crashing_before_push(&a, &["claim", &y, "--json"]).run();
        assert_eq!(later.returncode(), 98, "{}", later.stderr);
        let retry = a.run(retitle);
        assert_eq!(retry.returncode(), 0, "{}", retry.stderr);
        a.bead(["sync"]);
        b.bead(["sync"]);
        assert_eq!(
            b.show(&x).get("title"),
            Some(&Json::from("from-B")),
            "the accepted update was applied a second time, over the other machine's later title"
        );
    }
}
