//! G16: concurrent bead processes on one machine share no fetch destination: each fetch lands in a
//! ref of its own process (`docs/FINDINGS.md`).

mod g16_concurrent_processes_share_no_fetch_destination {
    use std::process::{Output, Stdio};
    use std::sync::mpsc::{self, Receiver};
    use std::time::Duration;

    use beadroll_test_support::{Invocation, Json, Machine, Run, parse_json};

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

        /// `communicate(timeout)`: how it ended and what it printed. Not ended within `timeout`,
        /// the test fails, as `TimeoutExpired` fails the Python test.
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

    /// The message of the error a failed `bead ... --json` reported: `json.loads(err)['error']
    /// ['message']`.
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
}
