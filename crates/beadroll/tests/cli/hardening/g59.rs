//! G59: help never shows a colour code as text. It is plain wherever nobody sees colour: through a
//! pipe, `FORCE_COLOR=1` included, in JSON, and on a terminal with `NO_COLOR`; on a terminal
//! otherwise it is coloured (roll-dih8.23). Its usage line says `Usage: bead create`.

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

/// The maximum duration of one bead invocation.
const LIMIT: Duration = Duration::from_secs(30);

/// `_g59_env`: `bead ARGS` from `machine`, its environment without the variables that turn colour
/// on or off (`NO_COLOR`, `FORCE_COLOR`, `CLICOLOR`, `CLICOLOR_FORCE`, `PYTHON_COLORS`, `TERM`),
/// then `name=value`.
fn command(machine: &Machine, args: &[&str], (name, value): (&str, &str)) -> Command {
    let mut command = machine.cmd(args).command();
    for colour in [
        "NO_COLOR",
        "FORCE_COLOR",
        "CLICOLOR",
        "CLICOLOR_FORCE",
        "PYTHON_COLORS",
        "TERM",
    ] {
        command.env_remove(colour);
    }
    command.env(name, value);
    command
}

/// The first 100 Unicode scalar values of `text`, for failure diagnostics.
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
/// [`LIMIT`] is killed and fails the test. Once it has started, the copies
/// of its standard streams that `command` holds close:
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

/// Runs `command` with nothing on standard input and its output piped, within [`LIMIT`].
fn run_through_pipes(mut command: Command) -> Run {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    finished(command)
}

/// `command` with standard input, output and error on a fresh
/// pseudo-terminal: no process's controlling terminal, and its
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
    // Start draining before bead starts so a full terminal buffer cannot block it.
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

    /// Machine `help`.
    fn set_up(sandbox: &Sandbox) -> Machine {
        Machine::new(sandbox, "help")
    }

    /// `text` without its colour codes (`ESC [ … m`).
    fn uncoloured(text: &str) -> String {
        let mut out = String::new();
        let mut rest = text;
        while let Some(at) = rest.find("\u{1b}[") {
            out.push_str(&rest[..at]);
            let after = &rest[at + 2..];
            let end = after
                .find(|c: char| !(c.is_ascii_digit() || c == ';'))
                .unwrap_or(after.len());
            assert!(
                after[end..].starts_with('m'),
                "not a colour code: {:?}",
                start(rest)
            );
            rest = &after[end + 1..];
        }
        out.push_str(rest);
        out
    }

    /// G59: `bead help create` and `bead create --help` on a terminal are coloured with codes the
    /// terminal reads, never shown as text; with `NO_COLOR` they are plain.
    #[test]
    fn help_on_a_terminal_is_coloured_unless_no_color_and_never_shows_a_code() {
        let sandbox = crate::sandbox();
        let machine = set_up(&sandbox);
        for args in [&["help", "create"][..], &["create", "--help"]] {
            let help = command(&machine, args, ("TERM", "xterm-256color"));
            let (status, out) = run_on_terminal(help);
            assert_eq!(status, 0, "{out}");
            assert!(
                !out.contains("\\x1b["),
                "{args:?} shows a code: {:?}",
                start(&out)
            );
            assert!(
                out.contains('\u{1b}'),
                "{args:?} is not coloured: {:?}",
                start(&out)
            );
            assert!(uncoloured(&out).contains("Usage: bead create"), "{out}");
            let mut plain = command(&machine, args, ("TERM", "xterm-256color"));
            plain.env("NO_COLOR", "1");
            let (status, out) = run_on_terminal(plain);
            assert_eq!(status, 0, "{out}");
            assert!(!out.contains('\u{1b}'), "{args:?}: {:?}", start(&out));
            assert!(out.contains("Usage: bead create"), "{out}");
        }
    }

    /// G59: with `FORCE_COLOR=1` (common in CI, and set by some agent harnesses), `help create`
    /// through a pipe shows no colour codes, escaped or raw, and `--json help create` embeds none
    /// in its help string: bead follows `CLICOLOR_FORCE` and `--color`, not `FORCE_COLOR`.
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
        assert!(stdout.contains("Usage: bead create"), "{stdout}");
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
        assert!(page.contains("Usage: bead create"), "{page}");
    }
}
