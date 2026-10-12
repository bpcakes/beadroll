//! A transport that wants the terminal cannot have bead's: git and ssh run in a session of their
//! own, so a sync fails fast instead of prompting.

use std::os::unix::fs::PermissionsExt as _;
use std::process::Stdio;
use std::time::{Duration, Instant};

use beadroll_test_support::Machine;

use super::{drain, quoted, script, wait_for};

/// Whether `needle` occurs in `haystack`.
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// Verified (V8): sync under a controlling terminal cannot prompt, because git and ssh run in their
/// own session (run_git uses start_new_session=True), so a transport that wants /dev/tty fails fast
/// instead of hanging. The review measured rc 1 after 0.44 s with bead as session leader of a PTY
/// and a fake ssh that opens /dev/tty for a passphrase: the terminal showed '/dev/tty: Device not
/// configured ... fatal: Could not read from remote repository.' and no prompt.
#[test]
fn sync_under_a_controlling_terminal_cannot_prompt() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let bindir = sandbox.root().join("fake-bin");
    std::fs::create_dir(&bindir).unwrap();
    // also first on PATH, so no real ssh can run whatever bead does to GIT_SSH_COMMAND
    let fake = bindir.join("ssh");
    let marker = sandbox.root().join("fake-ssh-ran");
    let program = format!(
        "#!/bin/sh\necho ran >> '{}'\nexec 3<>/dev/tty || {{ echo 'fake-ssh: no terminal' >&2; \
         exit 255; }}\nprintf 'Passphrase: ' >&3\nread x <&3\nexit 255\n",
        marker.display()
    );
    std::fs::write(&fake, program).unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut env = a.env(None, "codex");
    env.insert("GIT_SSH_COMMAND".into(), fake.into_os_string());
    env.insert("BEAD_NET_TIMEOUT".into(), "30".into());
    let mut path = bindir.into_os_string();
    path.push(":");
    path.push(std::env::var_os("PATH").unwrap_or_default());
    env.insert("PATH".into(), path);
    // bead is session leader; the terminal is its controlling terminal and standard input, output
    // and error.
    let line = format!(
        "exec {} sync --remote ssh://nowhere.invalid/x.git",
        quoted(a.executable().as_os_str())
    );
    let start = Instant::now();
    let mut child = script(&line, &env)
        .current_dir(a.checkout())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("script starts");
    // Nobody types on the terminal; script's input stays open, so it never types an end-of-file.
    let input = child.stdin.take();
    let output = drain(child.stdout.take().expect("the terminal's output"));
    let errors = drain(child.stderr.take().expect("script's errors"));
    let Some(status) = wait_for(&mut child, Duration::from_secs(80)) else {
        // closing the terminal ends bead too: its controlling terminal hangs up
        let _ = child.kill();
        let _ = child.wait();
        panic!("bead sync hung on a transport that wants the terminal");
    };
    let out = output
        .recv_timeout(Duration::from_secs(40))
        .expect("the terminal's output ends");
    let elapsed = start.elapsed();
    drop(input);
    let errors = errors
        .recv_timeout(Duration::from_secs(5))
        .unwrap_or_default();
    let shown = String::from_utf8_lossy(&out);
    assert!(marker.exists(), "the fake ssh never ran: {shown}");
    assert_eq!(
        status.code(),
        Some(1),
        "{shown}\nscript: {}",
        String::from_utf8_lossy(&errors)
    );
    assert!(elapsed < Duration::from_secs(15), "{elapsed:?}");
    assert!(!contains(&out, b"Passphrase:"), "{shown}");
    assert!(
        contains(&out, b"cannot reach the coordination remote"),
        "{shown}"
    );
}
