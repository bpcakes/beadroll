//! What bead prints on a terminal is what it prints into a pipe, and hostile text reaches neither
//! raw.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs::{File, OpenOptions};
use std::os::unix::fs::OpenOptionsExt as _;
use std::os::unix::process::ExitStatusExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use beadroll_test_support::{Machine, Run};
use rustix::process::{Pid, Signal, kill_process};

use super::{Drained, drain, quoted, script, wait_for};

/// A pseudo-terminal not controlled by bead. `script` owns the master and a placeholder sleep is
/// session leader; bead runs on the slave outside that session. A thread drains script's stdout.
/// Keep script's stdin pipe open to avoid an echoed end-of-file on the terminal.
struct Terminal {
    script: Child,
    input: Option<ChildStdin>,
    output: Option<Drained>,
    errors: Option<Drained>,
    /// The placeholder, until `close` ends it.
    holder: Option<Pid>,
    slave: PathBuf,
}

impl Terminal {
    /// A new terminal. The placeholder writes its pid and the slave's path to `ready`.
    fn open(ready: &Path) -> Terminal {
        let staged = ready.with_extension("staged");
        let line = format!(
            "echo \"$$ $(tty)\" > {staged} && mv {staged} {ready} && exec sleep 3600",
            staged = quoted(staged.as_os_str()),
            ready = quoted(ready.as_os_str()),
        );
        let env: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
        let mut child = script(&line, &env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("script starts");
        let mut terminal = Terminal {
            input: child.stdin.take(),
            output: Some(drain(child.stdout.take().expect("script's output"))),
            errors: Some(drain(child.stderr.take().expect("script's errors"))),
            script: child,
            holder: None,
            slave: PathBuf::new(),
        };
        let deadline = Instant::now() + Duration::from_secs(30);
        while !ready.exists() {
            let ended = terminal.script.try_wait().expect("script's status");
            if ended.is_some() || Instant::now() >= deadline {
                let errors = terminal.errors.take().expect("script's errors");
                let errors = errors
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap_or_default();
                panic!(
                    "script gave no terminal ({ended:?}): {}",
                    String::from_utf8_lossy(&errors)
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let named = std::fs::read_to_string(ready).expect("the placeholder's note");
        let (pid, slave) = named.trim().split_once(' ').expect("a pid and a terminal");
        let pid = pid.parse().expect("the placeholder's pid");
        terminal.holder = Some(Pid::from_raw(pid).expect("a pid"));
        terminal.slave = PathBuf::from(slave);
        terminal
    }

    /// The slave, for reading and writing, never as the test's controlling terminal.
    fn slave(&self) -> File {
        OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOCTTY)
            .open(&self.slave)
            .unwrap_or_else(|error| panic!("{}: {error}", self.slave.display()))
    }

    /// Everything written to the terminal. The placeholder ends, and with it the last hold on the
    /// slave once bead has ended too: script copies what is left and exits.
    fn close(mut self) -> Vec<u8> {
        let holder = self.holder.take().expect("an open terminal");
        kill_process(holder, Signal::KILL).expect("the placeholder ends");
        let ended = wait_for(&mut self.script, Duration::from_secs(60));
        assert!(ended.is_some(), "script went on after its terminal closed");
        self.input.take();
        let output = self.output.take().expect("script's output");
        output
            .recv_timeout(Duration::from_secs(60))
            .expect("script's output ends")
    }
}

impl Drop for Terminal {
    /// A test that failed half way leaves neither script nor its placeholder behind.
    fn drop(&mut self) {
        if let Ok(None) = self.script.try_wait() {
            if let Some(holder) = self.holder.take() {
                let _ = kill_process(holder, Signal::KILL);
            }
            let _ = self.script.kill();
            let _ = self.script.wait();
        }
    }
}

/// The exit code, or the negated signal number.
fn returncode(status: ExitStatus) -> i32 {
    status
        .code()
        .or_else(|| status.signal().map(|signal| -signal))
        .expect("an exit code or a signal")
}

/// `bytes` with every CRLF made LF: `bytes.replace(b"\r\n", b"\n")`.
fn lf(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut rest = bytes;
    while let Some((&first, tail)) = rest.split_first() {
        if first == b'\r' && tail.first() == Some(&b'\n') {
            out.push(b'\n');
            rest = &tail[1..];
        } else {
            out.push(first);
            rest = tail;
        }
    }
    out
}

/// A character the terminal must never receive raw: the test's `raw` pattern, C0 but LF, DEL and
/// C1, LS and PS, the directional marks, embeddings, overrides and isolates.
fn raw(c: char) -> bool {
    matches!(
        u32::from(c),
        0x00..=0x09
            | 0x0b..=0x1f
            | 0x7f..=0x9f
            | 0x2028
            | 0x2029
            | 0x200e
            | 0x200f
            | 0x202a..=0x202e
            | 0x2066..=0x2069
    )
}

/// Runs `bead ARGS` from `machine` with `env`, null input, piped output and a 60-second timeout:
/// returns how it ended and what it printed.
fn piped(
    machine: &Machine,
    args: &[String],
    env: &BTreeMap<OsString, OsString>,
) -> (ExitStatus, Vec<u8>, Vec<u8>) {
    let mut child = Command::new(machine.executable())
        .args(args)
        .current_dir(machine.checkout())
        .env_clear()
        .envs(env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("bead starts");
    let stdout = drain(child.stdout.take().expect("bead's output"));
    let stderr = drain(child.stderr.take().expect("bead's errors"));
    let Some(status) = wait_for(&mut child, Duration::from_secs(60)) else {
        let _ = child.kill();
        let _ = child.wait();
        panic!("bead {args:?} did not end within 60 s");
    };
    let limit = Duration::from_secs(60);
    let stdout = stdout.recv_timeout(limit).expect("bead's output ends");
    let stderr = stderr.recv_timeout(limit).expect("bead's errors end");
    (status, stdout, stderr)
}

/// `bead ARGS` from `machine` with `env`, its standard input and output on one new terminal and
/// its standard error on another (`ready` names their placeholders' notes): how it ended and what
/// each terminal showed.
fn on_terminals(
    machine: &Machine,
    args: &[String],
    env: &BTreeMap<OsString, OsString>,
    ready: &[PathBuf; 2],
) -> (ExitStatus, Vec<u8>, Vec<u8>) {
    let out_terminal = Terminal::open(&ready[0]);
    let err_terminal = Terminal::open(&ready[1]);
    let out_slave = out_terminal.slave();
    let mut command = Command::new(machine.executable());
    command
        .args(args)
        .current_dir(machine.checkout())
        .env_clear()
        .envs(env)
        .stdin(out_slave.try_clone().expect("the slave again"))
        .stdout(out_slave)
        .stderr(err_terminal.slave());
    let mut child = command.spawn().expect("bead starts");
    // The test keeps no hold on the slaves: only bead and the placeholders have them open.
    drop(command);
    let Some(status) = wait_for(&mut child, Duration::from_secs(60)) else {
        let _ = child.kill();
        let _ = child.wait();
        panic!("bead {args:?} did not end within 60 s on a terminal");
    };
    (status, out_terminal.close(), err_terminal.close())
}

/// Verified (V8): under a PTY (stdout and stderr on two separate PTYs) human output equals the
/// pipe output modulo CRLF, no raw C0/C1/bidi/LS/PS character reaches the terminal, errors go only
/// to stderr and exit codes match. The review ran 25 commands under 3 locales (75 runs) over
/// hostile titles, descriptions, labels, comments and a claim thread: exit codes equal 75/75,
/// stderr equal 75/75, stdout equal 69/75 (the 6 differences are `help create` / `create --help`,
/// finding ENV-2, not exercised here); only U+061C (ENV-3, not in this fixture) reached the PTY
/// raw; 0 tracebacks; all 15 error runs had empty stdout; the 5000-char and wide/combining titles
/// were byte-identical.
#[test]
fn pty_output_matches_pipe_output_and_escapes_hostile_text() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let long = "L".repeat(3000);
    let hostile = [
        "esc\x1b[31mRED\x1b]0;t\x07",
        "c1\u{9b}31m\u{85}",
        "bidi\u{202e}evil\u{2066}x\u{2069}",
        "tab\tX",
        "ls\u{2028}X\u{2029}",
        "cr\rX\x08",
        "nl\nFORGED",
        "\u{5bbd} e\u{301}\u{301} \u{1f469}\u{200d}\u{1f469}\u{200d}\u{1f467}",
        &long,
    ];
    let mut keys: Vec<String> = Vec::new();
    for title in hostile {
        let label = format!("l{}", keys.len());
        let created = a.bead(["create", title, "-d", title, "-l", &label]);
        let created = created.expect("the created issue");
        keys.push(created["id"].as_str().expect("an id").to_owned());
        a.bead(["comments", "add", &keys[keys.len() - 1], title]);
    }
    a.bead(["sync"]);
    a.cmd(["claim", &keys[2]])
        .thread("thr\u{202e}evil\x1b[2J")
        .json();
    let words = |items: &[&str]| items.iter().map(|w| (*w).to_owned()).collect::<Vec<_>>();
    let mut show = words(&["show"]);
    show.extend(keys.iter().cloned());
    let commands = [
        show,
        words(&["list", "--all"]),
        words(&["ready"]),
        words(&["claims"]),
        words(&["log"]),
        words(&["info"]),
        words(&["sync"]),
        words(&["comments", &keys[0]]),
        words(&["show", "ex-nope\x1b[31m"]),
        words(&["bogus\x1b]0;x\x07"]),
    ];
    // with colour off a terminal shows what a pipe does, byte for byte; with it on, the same once
    // bead's own colour codes are taken out, and nothing else raw (roll-dih8.23)
    let runs = commands
        .iter()
        .enumerate()
        .flat_map(|(index, args)| [(index, args, false), (index, args, true)]);
    for (index, args, coloured) in runs {
        let label = format!("{:?} coloured={coloured}", &args[..args.len().min(2)]);
        let env = environment(&a, coloured);
        let (pipe_status, pipe_stdout, pipe_stderr) = piped(&a, args, &env);
        let terminals = [
            sandbox
                .root()
                .join(format!("stdout-{index}-{coloured}.tty")),
            sandbox
                .root()
                .join(format!("stderr-{index}-{coloured}.tty")),
        ];
        let (status, out, err) = on_terminals(&a, args, &env, &terminals);
        if coloured && args[0] == "list" {
            assert!(out.contains(&0x1b), "{label}: not coloured on a terminal");
        }
        let (out, err) = (uncoloured(&out), uncoloured(&err));
        let shown = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
        assert_eq!(returncode(status), returncode(pipe_status), "{label}");
        assert!(
            lf(&out) == pipe_stdout,
            "{label}: standard output differs\n  terminal: {:?}\n  pipe:     {:?}",
            shown(&out),
            shown(&pipe_stdout)
        );
        assert!(
            lf(&err) == pipe_stderr,
            "{label}: standard error differs\n  terminal: {:?}\n  pipe:     {:?}",
            shown(&err),
            shown(&pipe_stderr)
        );
        if returncode(pipe_status) != 0 {
            assert!(
                out.is_empty(),
                "{label}: an error wrote to stdout: {:?}",
                shown(&out)
            );
            assert!(!err.is_empty(), "{label}");
        }
        let text = |bytes: Vec<u8>| {
            let text = String::from_utf8(bytes).expect("bead's output is UTF-8");
            text.replace("\r\n", "\n")
        };
        let text = text(out) + &text(err);
        let found: Vec<char> = text.chars().filter(|c| raw(*c)).collect();
        assert_eq!(found, Vec::<char>::new(), "{label}: {text:?}");
        assert!(!text.contains("\nFORGED"), "{label}: {text:?}");
        let run = Run {
            status,
            stdout: String::new(),
            stderr: text,
        };
        assert!(!run.crashed(), "{label}: {}", run.stderr);
    }
}

/// `machine`'s environment on a colour terminal, with colour on or turned off (`NO_COLOR`).
fn environment(machine: &Machine, coloured: bool) -> BTreeMap<OsString, OsString> {
    let mut env = machine.env(None, "codex");
    for colour in ["NO_COLOR", "CLICOLOR", "CLICOLOR_FORCE", "TERM"] {
        env.remove(OsStr::new(colour));
    }
    env.insert("TERM".into(), "xterm-256color".into());
    if !coloured {
        env.insert("NO_COLOR".into(), "1".into());
    }
    env
}

/// `bytes` without bead's colour codes (`ESC [`, digits and semicolons, `m`).
fn uncoloured(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == 0x1b && bytes.get(at + 1) == Some(&b'[') {
            let end = bytes[at + 2..]
                .iter()
                .position(|b| !(b.is_ascii_digit() || *b == b';'))
                .map(|n| at + 2 + n);
            if let Some(end) = end.filter(|end| bytes[*end] == b'm') {
                at = end + 1;
                continue;
            }
        }
        out.push(bytes[at]);
        at += 1;
    }
    out
}
