//! G29 (`docs/FINDINGS.md`): a failure to write after the remote accepted a command (`meta.json` on
//! a full volume) failed the command as `IO`, "nothing half-done", with its receipt already gone.
//! The Python tests run bead under a driver whose `write_atomic` raises ENOSPC for every write of
//! one kind of target throughout the command (`meta.json`, or another machine's segment); here
//! that is the failpoint `atomic-write:<part>@always=errno(ENOSPC)`, which only the Rust binary
//! has: `rust_only` tests. The lost acknowledgement before a retry is the crash hook's
//! (`BEAD_TEST_CRASH_AFTER_PUSH`, exit 99), as in Python.

/// G29: coordinated() drops the attempt and its receipt, then writes meta.json; publish_soon
/// catches BeadError only. So an OSError (a full volume) after the remote accepted a command is
/// reported as IO, which `bead prime` documents as "nothing half-done": the release or close has
/// happened, and the receipt that would let a retry recognise it is already gone.
mod g29_failure_after_acceptance_is_not_nothing_half_done {
    use beadroll_test_support::{Json, Machine, Run, Sandbox, parse_json};

    /// The failpoint path part of the workspace's `meta.json` (the driver's `G29_FULL=meta`).
    const META: &str = "/meta.json";

    /// The class's `setUp`: machine a, which initialised the tracker (prefix `gf`), and machine
    /// `obs`, which observes the remote.
    fn set_up(sandbox: &Sandbox) -> (Machine, Machine) {
        let (a, obs) = (
            crate::rust_only(sandbox, "a"),
            crate::rust_only(sandbox, "obs"),
        );
        a.bead(["init", "--prefix", "gf"]);
        (a, obs)
    }

    /// The id of the issue a `--json` result is: `result["id"]`.
    fn id(result: Option<Json>) -> String {
        let result = result.expect("bead printed a result");
        result["id"].as_str().expect("an id").to_owned()
    }

    /// `full`: `bead ARGV` on a, every write of a target whose path holds `part` failing with
    /// ENOSPC, as a full volume fails it.
    fn full(a: &Machine, part: &str, argv: &[&str]) -> Run {
        let failpoints = format!("atomic-write:{part}@always=errno(ENOSPC)");
        a.cmd(argv).env("BEAD_FAILPOINTS", failpoints).run()
    }

    /// `_g29_seen`: (status, owner) of the issue `key` as `machine` sees it.
    fn seen(machine: &Machine, key: &str) -> (String, Option<String>) {
        let issue = machine.show(key);
        let status = issue["status"].as_str().expect("a status").to_owned();
        let owner = issue["claim"]["owner"].as_str().map(str::to_owned);
        (status, owner)
    }

    /// `remote`: the issue `key` as the remote has it, which `obs` syncs to see.
    fn remote(obs: &Machine, key: &str) -> (String, Option<String>) {
        obs.bead(["sync"]);
        seen(obs, key)
    }

    /// The bytes of a's `meta.json`. While the failpoint holds, no write of it goes through: a
    /// command the remote accepted leaves it as it was, where the counts of what was published
    /// would otherwise have moved.
    fn meta(a: &Machine) -> Vec<u8> {
        std::fs::read(a.store().join("meta.json")).expect("a's meta.json")
    }

    /// What a run answered: its exit status and its error, for a message.
    fn answered(run: &Run) -> (i32, String) {
        (run.returncode(), run.stderr.trim().to_owned())
    }

    /// The id machine `m` has in its home's `replica.json`.
    fn replica(m: &Machine) -> String {
        let text = std::fs::read_to_string(m.home().join("replica.json")).expect("replica.json");
        let identity = parse_json(&text).unwrap_or_else(|error| panic!("{error}: {text}"));
        identity["id"].as_str().expect("a replica id").to_owned()
    }

    /// G29: the remote accepted a release / claiming close, then meta.json hit ENOSPC: not an IO
    /// failure.
    ///
    /// Replaces `G29_FailureAfterAcceptanceIsNotNothingHalfDone`'s
    /// `test_g29_accepted_command_is_not_reported_as_io_when_meta_cannot_be_written`, whose driver
    /// (`G29_FULL=meta`) fails every write of `meta.json`, as the failpoint on [`META`] does here.
    #[test]
    fn g29_accepted_command_is_not_reported_as_io_when_meta_cannot_be_written() {
        let sandbox = crate::sandbox();
        let (a, obs) = set_up(&sandbox);
        let released = id(a.bead(["create", "released"]));
        let closed = id(a.bead(["create", "closed"]));
        a.bead(["sync"]);
        a.bead(["claim", &released]);
        let cases: [(&str, Vec<&str>, &str); 2] = [
            ("release", vec!["release", &released], "open"),
            (
                "update --claim --status closed",
                vec!["update", &closed, "--claim", "--status", "closed"],
                "closed",
            ),
        ];
        for (name, argv, expected) in cases {
            let argv = [argv.as_slice(), &["--json"]].concat();
            let before = meta(&a);
            let first = full(&a, META, &argv);
            let kept = meta(&a) == before;
            let key = argv[1];
            assert_eq!(
                remote(&obs, key),
                (expected.to_owned(), None),
                "{name}: precondition: the remote has the {name}"
            );
            let retry = (first.returncode() != 0).then(|| answered(&a.run(&argv)));
            assert_eq!(
                first.returncode(),
                0,
                "{name} took effect on the remote, yet bead answered {}; its retry then answered \
                 {retry:?}",
                first.stderr.trim()
            );
            assert!(kept, "{name}: a write of meta.json went through");
        }
    }

    /// G29: a retry recognises its accepted release (the receipt), then meta.json hits ENOSPC: not
    /// IO.
    ///
    /// Replaces `G29_FailureAfterAcceptanceIsNotNothingHalfDone`'s
    /// `test_g29_recognised_retry_is_not_reported_as_io_when_meta_cannot_be_written`, whose driver
    /// (`G29_FULL=meta`) fails every write of `meta.json`, as the failpoint on [`META`] does here.
    #[test]
    fn g29_recognised_retry_is_not_reported_as_io_when_meta_cannot_be_written() {
        let sandbox = crate::sandbox();
        let (a, _) = set_up(&sandbox);
        let key = id(a.bead(["create", "x"]));
        a.bead(["sync"]);
        a.bead(["claim", &key]);
        let lost = a
            .cmd(["release", &key, "--json"])
            .env("BEAD_TEST_CRASH_AFTER_PUSH", "1")
            .run();
        assert_eq!(lost.returncode(), 99, "{}", lost.stderr);
        let before = meta(&a);
        let retry = full(&a, META, &["release", &key, "--json"]);
        let kept = meta(&a) == before;
        let again =
            (retry.returncode() != 0).then(|| answered(&a.run(["release", &key, "--json"])));
        assert_eq!(
            retry.returncode(),
            0,
            "the retry recognised the accepted release, yet answered {}; the next retry then \
             answered {again:?}",
            retry.stderr.trim()
        );
        assert!(kept, "a write of meta.json went through");
    }

    /// G29: the owner's close is recorded; its publish hits ENOSPC pulling another machine's
    /// segment: the answer is "recorded locally, not yet published", not exit 1 IO.
    ///
    /// Replaces `G29_FailureAfterAcceptanceIsNotNothingHalfDone`'s
    /// `test_g29_owner_close_whose_publish_hits_a_full_volume_is_recorded_not_failed`, whose
    /// driver (`G29_FULL=foreign`) fails every write of a segment outside a's own journal; machine
    /// b's is the only other journal there is, so the failpoint here names b's folder.
    #[test]
    fn g29_owner_close_whose_publish_hits_a_full_volume_is_recorded_not_failed() {
        let sandbox = crate::sandbox();
        let (a, _) = set_up(&sandbox);
        let key = id(a.bead(["create", "x"]));
        a.bead(["sync"]);
        a.bead(["claim", &key]);
        let b = crate::rust_only(&sandbox, "b");
        b.bead(["sync"]);
        b.bead(["create", "growth on b"]);
        b.bead(["sync"]);
        let foreign = format!("/events/{}/", replica(&b));
        let closed = full(&a, &foreign, &["close", &key, "--reason", "done"]);
        assert_eq!(
            seen(&a, &key),
            ("closed".to_owned(), None),
            "precondition: the close is recorded here"
        );
        let shown = if closed.stderr.trim().is_empty() {
            closed.stdout.trim()
        } else {
            closed.stderr.trim()
        };
        assert_eq!(
            (
                closed.returncode(),
                closed.stdout.contains("not yet published")
            ),
            (0, true),
            "the close was recorded, yet bead answered: {shown}"
        );
        // every write of b's journal failed: nothing of it reached a's store
        let pulled = a.store().join(&foreign[1..]);
        assert!(
            !pulled.exists() || std::fs::read_dir(&pulled).unwrap().next().is_none(),
            "a write of b's journal went through: {}",
            pulled.display()
        );
    }
}
