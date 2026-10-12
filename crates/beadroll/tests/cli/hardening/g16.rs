//! G16: concurrent bead processes on one machine share no fetch destination: each fetch lands in a
//! ref of its own process (`docs/FINDINGS.md`), and a fetch that loses to another process's
//! fetch, its ref gone while git read the refs, is tried again (roll-dih8.12).

mod g16_concurrent_processes_share_no_fetch_destination {
    use std::path::PathBuf;
    use std::process::{Output, Stdio};
    use std::sync::mpsc::{self, Receiver};
    use std::time::Duration;

    use beadroll_test_support::{Invocation, Json, Machine, Run, code, parse_json};

    use crate::hardening::gate::{Gate, beside};

    /// A `bead` started beside others, as `subprocess.Popen` starts one: nothing on its standard
    /// input, its output piped and read to the end by a thread of its own, which hands it over
    /// once the process has ended.
    struct Started(Receiver<std::io::Result<Output>>);

    impl Started {
        /// Starts `invocation`.
        fn start(invocation: Invocation<'_>) -> Started {
            let mut command = invocation.command();
            command
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let child = command.spawn().expect("bead starts");
            let (sender, receiver) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = sender.send(child.wait_with_output());
            });
            Started(receiver)
        }

        /// Waits for completion and captures output; exceeding `timeout` fails the test.
        fn communicate(self, timeout: Duration) -> Run {
            let output = self.0.recv_timeout(timeout);
            let output = output.unwrap_or_else(|_| panic!("bead had not ended after {timeout:?}"));
            let output = output.expect("bead's output");
            let text = |bytes: Vec<u8>| String::from_utf8(bytes).expect("bead's output is UTF-8");
            Run {
                status: output.status,
                stdout: text(output.stdout),
                stderr: text(output.stderr),
            }
        }
    }

    /// The message in a failed JSON command's error object.
    fn message(run: &Run) -> String {
        let document = parse_json(&run.stderr)
            .unwrap_or_else(|error| panic!("bead's error is not JSON ({error}): {}", run.stderr));
        let message = document.get("error").and_then(|e| e.get("message"));
        let message = message.and_then(Json::as_str);
        message
            .unwrap_or_else(|| panic!("no error message in {}", run.stderr))
            .to_owned()
    }

    /// G16: every bead process of a store fetches into the one refs/beads/remote, and git fetch
    /// updates it only if it still holds the value that fetch read first. Concurrent syncs and
    /// claims on one machine (agents in several worktrees share one store) fail with
    /// REMOTE_UNREACHABLE 'cannot reach the coordination remote ... cannot lock ref
    /// refs/beads/remote', although the remote was reachable and nothing contended on it.
    #[test]
    fn concurrent_syncs_and_claims_on_one_machine_do_not_fail_on_the_shared_remote_ref() {
        let sandbox = crate::sandbox();
        let a = Machine::new(&sandbox, "g16a");
        let b = Machine::new(&sandbox, "g16b");
        a.bead(["init", "--prefix", "ex"]);
        let keys: Vec<String> = (0..6)
            .map(|n| {
                let created = a.bead(["create", &format!("issue {n}")]).expect("an issue");
                created["id"].as_str().expect("the issue's id").to_owned()
            })
            .collect();
        a.bead(["sync"]);
        b.bead(["sync"]);
        let mut failures = Vec::new();
        for round_no in 0..3 {
            b.bead(["create", &format!("round {round_no}")]);
            // the remote moved: every fetch below updates the local copy of the remote head
            b.bead(["sync"]);
            let mut argvs: Vec<Vec<&str>> = vec![vec!["sync"]; 5];
            argvs.push(vec!["claim", &keys[2 * round_no]]);
            argvs.push(vec!["claim", &keys[2 * round_no + 1]]);
            let procs: Vec<Started> = argvs
                .iter()
                .enumerate()
                .map(|(i, argv)| {
                    let args = argv.iter().copied().chain(["--json"]);
                    Started::start(a.cmd(args).thread(format!("g16-t{i}")))
                })
                .collect();
            for (argv, proc) in argvs.iter().zip(procs) {
                let run = proc.communicate(Duration::from_secs(60));
                if run.returncode() != 0 {
                    failures.push(format!("{}: {}", argv[0], message(&run)));
                }
            }
        }
        assert_eq!(
            failures,
            Vec::<String>::new(),
            "{} of 21 concurrent commands on one machine failed",
            failures.len()
        );
        let exported = a.export();
        b.bead(["sync"]);
        assert_eq!(exported, b.export());
    }

    /// A ref among the fetch refs of `machine`'s `sync.git` that names no object: what git takes
    /// another process's fetch ref for when it is gone between listing it and reading it.
    fn broken_fetch_ref(machine: &Machine) -> PathBuf {
        let path = machine.store().join("sync.git/refs/beads/fetch/1-0badc0de");
        std::fs::create_dir_all(path.parent().expect("a directory")).expect("the fetch refs");
        std::fs::write(&path, format!("{}\n", "deadbeef".repeat(5))).expect("a broken ref");
        path
    }

    /// roll-dih8.12: git's checks of a fetch read every ref, so a fetch could find another
    /// process's fetch ref gone between listing it and reading it (`fatal: bad object
    /// refs/beads/fetch/...`), and the command failed with REMOTE_UNREACHABLE although the remote
    /// answered. A fetch ref that names no object stands in for that one: the fetch fails on it
    /// and waits to try again, the ref goes as the other process's does once its fetch is over,
    /// and the next attempt goes through.
    #[test]
    fn a_fetch_that_loses_to_another_fetch_tries_again() {
        let sandbox = crate::sandbox();
        let a = crate::rust_only(&sandbox, "g16-again");
        a.bead(["init", "--prefix", "ex"]);
        let other = broken_fetch_ref(&a);
        let gate = Gate::new(&sandbox, "g16-fetch-again");
        let failpoints = format!("fetch.retry={}", gate.action());
        let (synced, met) = beside(
            || {
                let sync = a.cmd(["sync", "--json"]);
                sync.env("BEAD_FAILPOINTS", &failpoints).run()
            },
            |finished| gate.meet(finished, || std::fs::remove_file(&other)),
        );
        let met = met.unwrap_or_else(|| panic!("no second attempt: {}", synced.stderr));
        met.expect("the other fetch's ref went");
        assert_eq!(synced.returncode(), 0, "{}", synced.stderr);
    }

    /// A fetch that loses every time is not tried for ever: after five attempts, four waits, it
    /// fails as it did before it was tried again, REMOTE_UNREACHABLE in git's own words.
    #[test]
    fn a_fetch_that_keeps_losing_gives_up_after_five_attempts() {
        let sandbox = crate::sandbox();
        let a = crate::rust_only(&sandbox, "g16-gives-up");
        a.bead(["init", "--prefix", "ex"]);
        broken_fetch_ref(&a);
        let waited = |nth: u32| {
            let failpoints = format!("fetch.retry@nth({nth})=exit(7)");
            a.cmd(["sync", "--json"])
                .env("BEAD_FAILPOINTS", failpoints)
                .run()
        };
        let fourth = waited(4);
        assert_eq!(fourth.returncode(), 7, "no fourth wait: {}", fourth.stderr);
        let given_up = waited(5);
        assert_eq!(code(&given_up), "REMOTE_UNREACHABLE", "{}", given_up.stderr);
        let said = message(&given_up);
        assert!(
            said.contains("bad object refs/beads/fetch/1-0badc0de"),
            "{said}"
        );
    }
}
