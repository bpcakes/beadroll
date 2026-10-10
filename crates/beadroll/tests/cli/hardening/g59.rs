//! G59: help is plain text. Python 3.14's argparse colours help on a terminal and under
//! `FORCE_COLOR`: `bead help create` showed its colour codes escaped (`\x1b[1;34m`), `bead create
//! --help` wrote raw SGR sequences, and `--json help create` carried them in its string.
//!
//! D-class (`docs/RUST-PORT.md` §4.6): the Python tests ran only where the `python3` that runs bead
//! has argparse's colour. These run whichever bead is under test: no ESC byte, raw or escaped,
//! reaches help on a terminal, with `FORCE_COLOR=1` or under `--json`, and the help says
//! `usage: bead create`.

use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use beadroll_test_support::{Machine, Run};
use rustix::io::{FdFlags, fcntl_setfd};
use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};

/// How long one run of bead may take, as the Python tests' `timeout=30`.
const LIMIT: Duration = Duration::from_secs(30);

/// `_g59_env`: `bead ARGS` from `machine`, its environment without the variables that turn colour
/// on or off (`NO_COLOR`, `FORCE_COLOR`, `PYTHON_COLORS`, `TERM`), then `name=value`.
fn command(machine: &Machine, args: &[&str], (name, value): (&str, &str)) -> Command {
    let mut command = machine.cmd(args).command();
    for colour in ["NO_COLOR", "FORCE_COLOR", "PYTHON_COLORS", "TERM"] {
        command.env_remove(colour);
    }
    command.env(name, value);
    command
}

/// The first 100 characters of `text`, which a failure quotes as the Python messages do.
fn start(text: &str) -> String {
    text.chars().take(100).collect()
}

/// A thread that reads `pipe` to its end: what it read.
fn read_all(mut pipe: impl Read + Send + 'static) -> JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = pipe.read_to_end(&mut bytes);
        bytes
    })
}

/// What a reader thread read, as `text=True` decodes it; nothing for a stream that was not piped.
fn text(reader: Option<JoinHandle<Vec<u8>>>) -> String {
    let bytes = reader.map(|thread| thread.join().expect("a reader"));
    String::from_utf8(bytes.unwrap_or_default()).expect("bead's output is UTF-8")
}

/// `command` run to its end, its piped output read on threads of their own; one still running after
/// [`LIMIT`] is killed and fails the test, as `timeout=30` raises. Once it has started, the copies
/// of its standard streams that `command` holds close, as `os.close(slave)` closes the terminal's:
/// a terminal reaches its end with bead.
fn finished(mut command: Command) -> Run {
    let mut child = command
        .spawn()
        .unwrap_or_else(|error| panic!("cannot start {command:?}: {error}"));
    drop(command);
    let stdout = child.stdout.take().map(read_all);
    let stderr = child.stderr.take().map(read_all);
    let deadline = Instant::now() + LIMIT;
    while child.try_wait().expect("bead's status").is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("bead ran longer than {LIMIT:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let status = child.wait().expect("bead's status");
    Run {
        status,
        stdout: text(stdout),
        stderr: text(stderr),
    }
}

/// `subprocess.run(..., capture_output=True, text=True, timeout=30)`: `command` with nothing on
/// standard input and its output piped.
fn run_through_pipes(mut command: Command) -> Run {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    finished(command)
}

/// `_g59_run_on_terminal`: `command` with standard input, output and error on a fresh
/// pseudo-terminal, made as `os.openpty()` makes one: no process's controlling terminal, and its
/// master not inherited. How it exited, and what it wrote there, decoded with replacement and with
/// CRLF read as LF.
fn run_on_terminal(mut command: Command) -> (i32, String) {
    let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).expect("a pseudo-terminal");
    fcntl_setfd(&master, FdFlags::CLOEXEC).expect("the master closes on exec");
    grantpt(&master).expect("grantpt");
    unlockpt(&master).expect("unlockpt");
    let name = ptsname(&master, Vec::new()).expect("the terminal's name");
    let path = Path::new(OsStr::from_bytes(name.as_bytes()));
    let terminal = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOCTTY)
        .open(path)
        .unwrap_or_else(|error| panic!("cannot open the terminal {}: {error}", path.display()));
    // read from before bead starts, as the Python test's drain thread is
    let written = drained(master);
    let input = terminal.try_clone().expect("the terminal, again");
    let output = terminal.try_clone().expect("the terminal, again");
    command.stdin(input).stdout(output).stderr(terminal);
    let status = finished(command).returncode();
    let bytes = written
        .recv_timeout(LIMIT)
        .expect("the terminal reaches its end once bead has exited");
    let out = String::from_utf8_lossy(&bytes).replace("\r\n", "\n");
    (status, out)
}

/// Everything written to the terminal whose master is `master`, handed over once no process holds
/// the terminal any more: a read of the master then fails with EIO (Linux) or ends (macOS), and
/// what came before stays read.
fn drained(master: OwnedFd) -> Receiver<Vec<u8>> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = File::from(master).read_to_end(&mut bytes);
        let _ = sender.send(bytes);
    });
    receiver
}

mod g59_help_is_plain_text {
    use beadroll_test_support::{Machine, Sandbox, parse_json};

    use super::{command, run_on_terminal, run_through_pipes, start};

    /// The class's `setUp`: machine `help`.
    fn set_up(sandbox: &Sandbox) -> Machine {
        Machine::new(sandbox, "help")
    }

    /// G59: `bead help create` on a terminal shows no colour codes, neither as the text
    /// `\x1b[1;34m` (Python 3.14's argparse colours, escaped by `visible()`) nor raw, and says
    /// `usage: bead create`.
    #[test]
    fn help_command_on_a_terminal_shows_no_colour_codes() {
        let sandbox = crate::sandbox();
        let machine = set_up(&sandbox);
        let help = command(&machine, &["help", "create"], ("TERM", "xterm-256color"));
        let (status, out) = run_on_terminal(help);
        assert_eq!(status, 0, "{out}");
        assert!(
            !out.contains("\\x1b["),
            "help on a terminal shows escaped colour codes: {:?}",
            start(&out)
        );
        assert!(!out.contains('\u{1b}'), "{:?}", start(&out));
        assert!(out.contains("usage: bead create"), "{out}");
    }

    /// G59: `bead create --help` on a terminal writes no escape sequence, where Python 3.14's
    /// argparse wrote raw SGR sequences, unlike every other bead output.
    #[test]
    fn command_dash_help_on_a_terminal_writes_no_escape_sequences() {
        let sandbox = crate::sandbox();
        let machine = set_up(&sandbox);
        let help = command(&machine, &["create", "--help"], ("TERM", "xterm-256color"));
        let (status, out) = run_on_terminal(help);
        assert_eq!(status, 0, "{out}");
        assert!(
            !out.contains('\u{1b}'),
            "create --help on a terminal wrote escape sequences: {:?}",
            start(&out)
        );
        assert!(out.contains("usage: bead create"), "{out}");
    }

    /// G59: with `FORCE_COLOR=1` (common in CI), `help create` through a pipe shows no colour
    /// codes, escaped or raw, and `--json help create` embeds none in its help string.
    #[test]
    fn forced_colour_reaches_neither_help_text_nor_json() {
        let sandbox = crate::sandbox();
        let machine = set_up(&sandbox);
        let forced = ("FORCE_COLOR", "1");
        let text = run_through_pipes(command(&machine, &["help", "create"], forced));
        assert_eq!(text.returncode(), 0, "{}", text.stderr);
        let stdout = &text.stdout;
        assert!(
            !stdout.contains("\\x1b["),
            "help through a pipe shows escaped colour codes: {:?}",
            start(stdout)
        );
        assert!(!stdout.contains('\u{1b}'), "{:?}", start(stdout));
        assert!(stdout.contains("usage: bead create"), "{stdout}");
        let as_json = run_through_pipes(command(&machine, &["--json", "help", "create"], forced));
        assert_eq!(as_json.returncode(), 0, "{}", as_json.stderr);
        let document = parse_json(&as_json.stdout).unwrap();
        let page = document["help"].as_str().expect("the help page");
        assert_eq!(
            page.matches('\u{1b}').count(),
            0,
            "--json help carries ANSI codes: {:?}",
            start(page)
        );
        assert!(page.contains("usage: bead create"), "{page}");
    }
}
