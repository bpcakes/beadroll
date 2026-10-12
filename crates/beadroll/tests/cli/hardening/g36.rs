//! G36: a terminal that hung up (its window closed, the ssh session that owned it gone) and
//! descriptors closed at start: a command that committed exits 0 quietly, a reading command too,
//! and a command that failed keeps its own status.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write as _};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use beadroll_test_support::Run;
use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};

/// The slave side of a pseudo-terminal whose master is closed. Writes fail with EIO, as after a
/// terminal window or SSH session disappears. It is opened without becoming a controlling
/// terminal.
fn hung_up_terminal() -> File {
    let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).expect("a pseudo-terminal");
    grantpt(&master).expect("grantpt");
    unlockpt(&master).expect("unlockpt");
    let name = ptsname(&master, Vec::new()).expect("the terminal's name");
    let name = Path::new(std::ffi::OsStr::from_bytes(name.as_bytes())).to_path_buf();
    let terminal = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOCTTY)
        .open(&name)
        .unwrap_or_else(|error| panic!("cannot open the terminal {}: {error}", name.display()));
    // the master closes: the terminal hangs up
    drop(master);
    let error = (&terminal).write_all(b"x");
    let error = error.expect_err("a write to a hung-up terminal fails");
    assert_eq!(
        error.raw_os_error(),
        Some(libc::EIO),
        "{}: {error}",
        name.display()
    );
    terminal
}

/// `_g36_noise`: how a run ended, and the last 300 characters of its standard error.
fn noise(proc: &Run) -> String {
    let stderr = &proc.stderr;
    let from_end = stderr.char_indices().rev().nth(299);
    let start = from_end.map_or(0, |(at, _)| at);
    format!(
        "exit {}, stderr ends: {:?}",
        proc.returncode(),
        &stderr[start..]
    )
}

/// A thread that reads `pipe` to its end: what it read.
fn drain(mut pipe: impl Read + Send + 'static) -> JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = pipe.read_to_end(&mut bytes);
        bytes
    })
}

/// What a reader thread read, decoded as `bytes.decode(errors="replace")` decodes it; nothing for a
/// stream that was not piped.
fn decoded(reader: Option<JoinHandle<Vec<u8>>>) -> String {
    let bytes = reader.map(|thread| thread.join().expect("a reader"));
    let bytes = bytes.unwrap_or_default();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// `command` run to the end with nothing on its standard input, its piped output read on threads
/// of their own. As `subprocess.run(..., timeout=30)` raises, a command that runs longer than
/// `limit` is killed and fails the test.
fn run_within(mut command: Command, limit: Duration) -> Run {
    let mut child = command.stdin(Stdio::null()).spawn().expect("bead starts");
    let stdout = child.stdout.take().map(drain);
    let stderr = child.stderr.take().map(drain);
    let deadline = Instant::now() + limit;
    let mut late = false;
    while child.try_wait().expect("bead's status").is_none() {
        if Instant::now() > deadline {
            late = true;
            let _ = child.kill();
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let status = child.wait().expect("bead's status");
    assert!(!late, "{command:?} ran longer than {limit:?}");
    Run {
        status,
        stdout: decoded(stdout),
        stderr: decoded(stderr),
    }
}

mod g36_hung_up_terminal_and_closed_descriptors {
    use std::process::{Command, Stdio};
    use std::time::Duration;

    use beadroll_test_support::{Machine, Run, Sandbox};

    use super::{hung_up_terminal, noise, run_within};

    /// The class's `setUp`: machine `tty`, its tracker initialised with the prefix `ex`.
    fn setup(sandbox: &Sandbox) -> Machine {
        let machine = Machine::new(sandbox, "tty");
        machine.bead(["init", "--prefix", "ex"]);
        machine
    }

    /// `run_bead`: bead ARGS in the machine's checkout with its environment, nothing on standard
    /// input, and standard output and error as given; at most 30 seconds.
    fn run_bead(machine: &Machine, args: &[&str], stdout: Stdio, stderr: Stdio) -> Run {
        let mut command = machine.cmd(args).command();
        command.stdout(stdout).stderr(stderr);
        run_within(command, Duration::from_secs(30))
    }

    /// `run_with_closed`: bead run as `bead ARGS >&-` (or `2>&-`), started with that descriptor
    /// closed; at most 30 seconds.
    fn run_with_closed(machine: &Machine, redirection: &str, args: &[&str]) -> Run {
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg(format!("exec \"$0\" \"$@\" {redirection}"))
            .arg(machine.executable())
            .args(args)
            .current_dir(machine.checkout())
            .env_clear()
            .envs(machine.env(None, "codex"))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        run_within(command, Duration::from_secs(30))
    }

    /// `titles`: the title of every issue, as `bead list --all --limit 0` lists them.
    fn titles(machine: &Machine) -> Vec<String> {
        let listed = machine.bead(["list", "--all", "--limit", "0"]).unwrap();
        let issues = listed["issues"].as_array().expect("the listed issues");
        issues
            .iter()
            .map(|issue| issue["title"].as_str().expect("a title").to_owned())
            .collect()
    }

    /// G36: a create that committed exits 120 with an EIO traceback when its terminal hung up, so
    /// a wrapper that retries on failure creates the issue twice; EPIPE on a closed pipe already
    /// exits 0 quietly.
    #[test]
    fn committed_create_on_a_hung_up_terminal_exits_zero_quietly() {
        let sandbox = crate::sandbox();
        let machine = setup(&sandbox);
        let cases: [&[&str]; 2] = [
            &["create", "made while the terminal was gone"],
            &["create", "made as json", "--json"],
        ];
        for args in cases {
            let terminal = hung_up_terminal();
            let proc = run_bead(&machine, args, Stdio::from(terminal), Stdio::piped());
            // the write committed
            let listed = titles(&machine);
            assert!(
                listed.iter().any(|title| title == args[1]),
                "{args:?}: {listed:?}"
            );
            assert!(!proc.crashed(), "{args:?}: {}", noise(&proc));
            assert_eq!(proc.returncode(), 0, "{args:?}: {}", noise(&proc));
        }
    }

    /// G36: list, JSON list, overview, export and command help must exit quietly with status 0
    /// when the terminal hangs up.
    #[test]
    fn read_commands_on_a_hung_up_terminal_exit_zero_quietly() {
        let sandbox = crate::sandbox();
        let machine = setup(&sandbox);
        machine.bead(["create", "something to list"]);
        let cases: [&[&str]; 5] = [
            &["list"],
            &["list", "--json"],
            &[],
            &["export"],
            &["create", "--help"],
        ];
        for args in cases {
            let terminal = hung_up_terminal();
            let proc = run_bead(&machine, args, Stdio::from(terminal), Stdio::piped());
            assert!(!proc.crashed(), "{args:?}: {}", noise(&proc));
            assert!(
                !proc.stderr.contains("Exception ignored"),
                "{args:?}: {}",
                noise(&proc)
            );
            assert_eq!(proc.returncode(), 0, "{args:?}: {}", noise(&proc));
        }
    }

    /// G36: a failed command whose terminal hung up exits 120, not its own status (1, or 2 for
    /// USAGE), which F06 decided for a closed stderr.
    #[test]
    fn failed_command_on_a_hung_up_terminal_keeps_its_status() {
        let sandbox = crate::sandbox();
        let machine = setup(&sandbox);
        let cases: [(&[&str], i32); 2] = [(&["show", "ex-nope"], 1), (&["no-such-command"], 2)];
        for (args, status) in cases {
            let terminal = hung_up_terminal();
            // standard output and error on the one terminal, as `stdout=slave, stderr=slave`
            let errors = terminal.try_clone().expect("the terminal, again");
            let proc = run_bead(&machine, args, Stdio::from(terminal), Stdio::from(errors));
            assert_eq!(
                proc.returncode(),
                status,
                "`bead {}` on a hung-up terminal exited {}, not its own status {status}",
                args.join(" "),
                proc.returncode()
            );
        }
    }

    /// G36: started with stdout closed (`>&-`), a committed create exits 1 with an AttributeError
    /// traceback; started with stderr closed, a usage error exits 1 instead of 2 and prints its
    /// message on stdout.
    #[test]
    fn closed_descriptors_do_not_turn_a_committed_create_into_a_traceback() {
        let sandbox = crate::sandbox();
        let machine = setup(&sandbox);
        let made = run_with_closed(&machine, ">&-", &["create", "made with stdout closed"]);
        // the write committed
        assert_eq!(titles(&machine), ["made with stdout closed"]);
        assert!(!made.crashed(), "{}", noise(&made));
        assert_eq!(made.returncode(), 0, "{}", noise(&made));
        let usage = run_with_closed(&machine, "2>&-", &["no-such-command"]);
        assert_eq!(
            (usage.returncode(), usage.stdout.as_str()),
            (2, ""),
            "a usage error with stderr closed exited {} (USAGE is 2) and wrote {:?} on stdout",
            usage.returncode(),
            usage.stdout
        );
    }
}
