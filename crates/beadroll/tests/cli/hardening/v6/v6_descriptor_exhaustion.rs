//! `V6_DescriptorExhaustion`: running out of file descriptors never drops a pending attempt, and
//! fails without a crash. The limits are applied through `/bin/sh`'s `ulimit -n`, as in the Python
//! test; without a `/bin/sh` the class skips, as the Python one does.

use beadroll_test_support::Machine;

use super::{error_code, id, limited, with_sh};

/// The descriptor limits the test runs: the Python test's own, 8, 10 and 11, and the same three two
/// descriptors higher for Rust's bead, 10, 12 and 13.
const LIMITS: [u32; 5] = [8, 10, 11, 12, 13];

/// The suffixes of the files in `machine`'s `pending/`, sorted: `sorted(p.suffix for p in
/// (store / "pending").iterdir())`.
fn pending_suffixes(machine: &Machine) -> Vec<String> {
    let entries = std::fs::read_dir(machine.store().join("pending")).unwrap();
    let mut suffixes: Vec<String> = entries
        .map(|entry| {
            let name = entry.unwrap().file_name();
            let name = name.to_string_lossy();
            // pathlib's suffix: from the last dot, unless that dot begins the name
            match name.rfind('.') {
                Some(dot) if dot > 0 => name[dot..].to_owned(),
                _ => String::new(),
            }
        })
        .collect();
    suffixes.sort();
    suffixes
}

/// Verified (V6): running out of file descriptors never drops a pending attempt and fails
/// without a traceback. The review measured, after a lost-ack `update --claim --status closed`
/// (crash after push): under `ulimit -n` 8, 9, 10 and 11, `show X`, `claim Y` and `sync` leave the
/// pending .json and .command both in place, and the retry exits 0. (At -n 6/8 everything is IO
/// 'Too many open files'; at -n >= 12 commands succeed.) This test runs limits 8, 10 and 11.
///
/// Adapted on closing G58: git's output is captured through files rather than pipes, so `claim` and
/// `sync` get by with fewer descriptors and may now succeed at -n 10/11, in which case they confirm
/// the attempt (the remote holds it) rather than leave it pending; either way it is never dropped.
/// Once a later command of the actor has gone through, the retry of the lost one is answered on its
/// merits (x is closed).
///
/// Recalibrated for Rust's bead (`docs/RUST-PORT.md` §4.4), which needs more descriptors than
/// Python's to reach the same paths. Measured on macOS, Python's bead reads the tracker (`show`)
/// from -n 8 and gets `claim` and `sync` through from -n 10; Rust's reads it from -n 10 and gets
/// them through from -n 11, and below 10 every command fails. So the test runs the Python test's
/// limits, 8, 10 and 11, and the same limits two descriptors higher, 10, 12 and 13. Each binary
/// meets its descriptors running out between the read and the coordinated commands (Python at 8,
/// Rust at 10) and enough of them for everything (Python from 10, Rust from 11); at 8, Rust's also
/// run out before anything is read, as Python's did at 8 when the review measured it.
#[test]
fn descriptor_exhaustion_never_drops_a_pending_attempt() {
    if !with_sh() {
        return;
    }
    for limit in LIMITS {
        let sandbox = crate::sandbox();
        let a = Machine::new(&sandbox, &format!("fd{limit}"));
        a.bead(["init", "--prefix", "ex"]);
        let x = id(a.bead(["create", "x"]));
        let y = id(a.bead(["create", "y"]));
        a.bead(["sync"]);
        let argv = ["update", x.as_str(), "--claim", "--status", "closed"];
        let lost = a
            .cmd(argv.iter().chain(&["--json"]))
            .env("BEAD_TEST_CRASH_AFTER_PUSH", "1")
            .run();
        assert_eq!(lost.returncode(), 99, "{}", lost.stderr);
        let mut confirmed = false;
        let commands: [&[&str]; 3] = [&["show", &x], &["claim", &y], &["sync"]];
        for cmd in commands {
            let proc = limited(&a, &format!("-n {limit}"), cmd);
            assert!(
                !proc.crashed(),
                "ulimit -n {limit} {}: {}",
                cmd[0],
                proc.stderr
            );
            let pending = pending_suffixes(&a);
            if pending != [".command", ".json"] {
                // settled, then: the attempt is confirmed (x was open)
                let shown = a.show(&x);
                assert_eq!(
                    (pending, proc.returncode(), shown["status"].as_str()),
                    (Vec::new(), 0, Some("closed")),
                    "ulimit -n {limit} {}: {}",
                    cmd[0],
                    proc.stderr
                );
                confirmed = true;
            }
        }
        let retry = a.run(argv.iter().chain(&["--json"]));
        assert!(
            retry.returncode() == 0
                || (confirmed && error_code(&retry).as_deref() == Some("VALIDATION")),
            "ulimit -n {limit}: {}",
            retry.stderr
        );
        assert_eq!(
            a.show(&x)["status"].as_str(),
            Some("closed"),
            "ulimit -n {limit}"
        );
    }
}
