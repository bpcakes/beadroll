//! A controlling terminal that hangs up in the middle of bead's output ends bead by SIGHUP,
//! quietly.
//!
//! The Python test holds the terminal's master itself: it reads at least 1 KiB, closes the master
//! while bead is blocked writing the rest, and reads bead's status. A test without `unsafe` gets
//! the terminal from a tool, and that tool must close the master while bead's status can still be
//! read, before bead has written all its 61 KB. `script(1)` alone cannot: it closes the master only
//! by exiting, which leaves bead to init, and it copies the terminal into a pipe that holds all of
//! bead's output. On macOS the terminal comes from `expect`, which macOS ships: it reads the master
//! itself, closes it and then waits for bead. Linux may not have `expect`; there `script` holds
//! the terminal and copies it into a pipe the test has filled but for one page, so that it and bead
//! block behind it, and the test, a subreaper, kills `script` once a KiB of bead's output has come
//! through: the master closes, and bead falls to the test.

use beadroll_test_support::{Machine, Sandbox};

/// Verified (V8): closing the controlling terminal mid-output ends bead quietly by SIGHUP, with no
/// traceback. The review ran `list --all --limit 0` over 40-60 issues with 1500-char titles and
/// closed the master after reading 1 KB: rc -1 (-SIGHUP), nothing written. (With SIGHUP ignored,
/// or without a controlling terminal, the result is ENV-1's EIO path instead, so SIGHUP is reset to
/// its default here.)
#[test]
fn hangup_of_the_controlling_terminal_ends_bead_quietly() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    for i in 0..40 {
        a.bead(["create", &format!("bulk {i} {}", "y".repeat(1500))]);
    }
    let (got, ended) = hang_up(&sandbox, &a, &["list", "--all", "--limit", "0"]);
    let shown = String::from_utf8_lossy(&got[got.len().saturating_sub(300)..]).into_owned();
    assert!(got.len() >= 1024, "{shown}");
    assert_eq!(ended, "killed by SIGHUP", "{shown}");
    let crashed = |needle: &[u8]| got.windows(needle.len()).any(|w| w == needle);
    assert!(
        !crashed(b"Traceback") && !crashed(b"panicked at"),
        "{shown}"
    );
}

/// `expect`'s part: bead spawned as the session leader on a new terminal, every signal at its
/// default as `spawn` leaves them (SIGHUP included: the Python test's preexec), its output read
/// until at least 1024 bytes came or 30 seconds passed (or bead ended), the bytes written to the
/// file `argv[0]`, the master closed, and bead's end printed as `wait` reports it.
#[cfg(not(target_os = "linux"))]
const EXPECT: &str = r#"log_user 0
set got_file [lindex $argv 0]
set deadline [expr {[clock seconds] + 30}]
spawn -noecho {*}[lrange $argv 1 end]
fconfigure $spawn_id -translation binary
set got ""
while {[string length $got] < 1024 && [clock seconds] < $deadline} {
    set timeout [expr {$deadline - [clock seconds]}]
    expect {
        -re {.+} { append got $expect_out(0,string) }
        timeout { break }
        eof { break }
    }
}
set out [open $got_file w]
fconfigure $out -translation binary
puts -nonewline $out $got
close $out
close
puts [wait]
"#;

/// bead ARGS from `machine` on a terminal that hangs up after its first KiB: what the terminal
/// showed until then, and how bead ended ("killed by SIGNAME" or "exited with N").
#[cfg(not(target_os = "linux"))]
fn hang_up(sandbox: &Sandbox, machine: &Machine, args: &[&str]) -> (Vec<u8>, String) {
    use std::process::{Command, Stdio};
    use std::time::Duration;

    use super::{drain, wait_for};

    let program = sandbox.root().join("hangup.exp");
    std::fs::write(&program, EXPECT).unwrap();
    let got_file = sandbox.root().join("hangup.got");
    let mut child = Command::new("expect")
        .arg(&program)
        .arg(&got_file)
        .arg(machine.executable())
        .args(args)
        .current_dir(machine.checkout())
        .env_clear()
        .envs(machine.env(None, "codex"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("expect starts (macOS ships it as /usr/bin/expect)");
    let output = drain(child.stdout.take().expect("expect's output"));
    let errors = drain(child.stderr.take().expect("expect's errors"));
    let Some(status) = wait_for(&mut child, Duration::from_secs(90)) else {
        let _ = child.kill();
        let _ = child.wait();
        panic!("bead did not end after its terminal hung up");
    };
    let limit = Duration::from_secs(30);
    let reported = output.recv_timeout(limit).expect("expect's output ends");
    let errors = errors.recv_timeout(limit).expect("expect's errors end");
    let reported = String::from_utf8_lossy(&reported).into_owned();
    assert!(
        status.success(),
        "expect failed: {}",
        String::from_utf8_lossy(&errors)
    );
    // `wait` prints the pid, the spawn id, 0 and the exit status, then for a signal CHILDKILLED,
    // its name and a description.
    let words: Vec<&str> = reported.split_whitespace().collect();
    let ended = match words.as_slice() {
        [_, _, "0", _, "CHILDKILLED", signal, ..] => format!("killed by {signal}"),
        [_, _, "0", code] => format!("exited with {code}"),
        _ => panic!("expect reported {reported:?}"),
    };
    let got = std::fs::read(&got_file).expect("what the terminal showed");
    (got, ended)
}

/// bead ARGS from `machine` on a terminal that hangs up after its first KiB: what the terminal
/// showed until then, and how bead ended ("killed by SIGNAME" or "exited with N"). `script` holds
/// the terminal, with bead as its session leader and SIGHUP at its default for it (`env
/// --default-signal`, the Python test's preexec), and copies it into a pipe that the test filled
/// beforehand but for one page: the pipe takes at most a page of bead's output, `script` then
/// blocks and the terminal fills, so bead is still writing when the test, having seen a KiB of it
/// arrive, kills `script` and with it the master. bead, whose parent is gone, falls to the test,
/// a subreaper, which reads its status.
#[cfg(target_os = "linux")]
fn hang_up(sandbox: &Sandbox, machine: &Machine, args: &[&str]) -> (Vec<u8>, String) {
    use std::ffi::OsStr;
    use std::io::{ErrorKind, Read as _, Write as _};
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    use rustix::io::{ioctl_fionbio, ioctl_fionread};
    use rustix::process::{
        Pid, Signal, WaitOptions, getpid, kill_process, set_child_subreaper, waitpid,
    };

    use super::{drain, quoted, script, wait_for};

    set_child_subreaper(Some(getpid())).expect("the test becomes a subreaper");
    let (mut reader, mut writer) = std::io::pipe().expect("a pipe");
    ioctl_fionbio(&writer, true).expect("a non-blocking pipe");
    let page = [b'-'; 4096];
    let mut filler = 0;
    loop {
        match writer.write(&page) {
            Ok(written) => filler += written,
            Err(error) if error.kind() == ErrorKind::WouldBlock => break,
            Err(error) => panic!("the pipe: {error}"),
        }
    }
    // One page read frees one of the pipe's pages: room for 4096 bytes, and no more.
    let mut first = vec![0; page.len()];
    reader.read_exact(&mut first).expect("the filler");
    filler -= page.len();
    ioctl_fionbio(&writer, false).expect("a blocking pipe again");
    let pid_file = sandbox.root().join("hangup.pid");
    let words: Vec<String> = std::iter::once(machine.executable().as_os_str())
        .chain(args.iter().map(OsStr::new))
        .map(quoted)
        .collect();
    let line = format!(
        "echo $$ > {}; exec env --default-signal=HUP {}",
        quoted(pid_file.as_os_str()),
        words.join(" ")
    );
    let mut command = script(&line, &machine.env(None, "codex"));
    command
        .current_dir(machine.checkout())
        // nobody types on the terminal; an open input keeps script from typing an end-of-file
        .stdin(Stdio::piped())
        .stdout(writer)
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("script starts");
    // Only script holds the pipe's writing end: once it is gone, the pipe ends.
    drop(command);
    let input = child.stdin.take();
    let errors = drain(child.stderr.take().expect("script's errors"));
    let deadline = Instant::now() + Duration::from_secs(30);
    let early = loop {
        let queued = ioctl_fionread(&reader).expect("the pipe's content");
        let came = usize::try_from(queued).expect("a size") - filler;
        let early = child.try_wait().expect("script's status");
        if came >= 1024 || early.is_some() || Instant::now() >= deadline {
            break early;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    if let Some(status) = early {
        let mut copied = Vec::new();
        reader.read_to_end(&mut copied).expect("the pipe");
        let copied = String::from_utf8_lossy(&copied[filler.min(copied.len())..]).into_owned();
        panic!("bead ended before its terminal hung up (script: {status}): {copied}");
    }
    // the terminal window closes
    kill_process(Pid::from_child(&child), Signal::KILL).expect("script ends");
    let ended = wait_for(&mut child, Duration::from_secs(60));
    assert!(ended.is_some(), "script outlived SIGKILL");
    drop(input);
    let pid = std::fs::read_to_string(&pid_file).unwrap_or_else(|error| {
        let errors = errors.recv_timeout(Duration::from_secs(5));
        let errors = errors.unwrap_or_default();
        panic!(
            "bead never started ({error}): {}",
            String::from_utf8_lossy(&errors)
        )
    });
    let pid = Pid::from_raw(pid.trim().parse().expect("bead's pid")).expect("a pid");
    let deadline = Instant::now() + Duration::from_secs(60);
    let status = loop {
        let reaped = waitpid(Some(pid), WaitOptions::NOHANG).expect("bead, fallen to the test");
        if let Some((_, status)) = reaped {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "bead did not end after its terminal hung up"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    let ended = match (status.terminating_signal(), status.exit_status()) {
        (Some(signal), _) => format!("killed by {}", signal_name(signal)),
        (None, Some(code)) => format!("exited with {code}"),
        (None, None) => panic!("bead neither exited nor was killed: {status:?}"),
    };
    let mut copied = Vec::new();
    reader.read_to_end(&mut copied).expect("the pipe");
    (copied.split_off(filler), ended)
}

/// The name of signal `number`, as `expect` names it.
#[cfg(target_os = "linux")]
fn signal_name(number: i32) -> String {
    match number {
        1 => "SIGHUP".to_owned(),
        6 => "SIGABRT".to_owned(),
        9 => "SIGKILL".to_owned(),
        11 => "SIGSEGV".to_owned(),
        13 => "SIGPIPE".to_owned(),
        15 => "SIGTERM".to_owned(),
        other => format!("signal {other}"),
    }
}
