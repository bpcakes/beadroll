//! V6: descriptor exhaustion must return an error without losing a pending attempt. Apply limits
//! with `/bin/sh` and `ulimit -n`; skip if the shell is unavailable.

use beadroll_test_support::Machine;

use super::{error_code, id, limited, with_sh};

/// Descriptor limits spanning failure before reading, failure during coordination and sufficient
/// capacity: 8, 10, 11, 10, 12 and 13.
const LIMITS: [u32; 5] = [8, 10, 11, 12, 13];

/// The suffixes of the files in `machine`'s `pending/`, sorted: `sorted(p.suffix for p in
/// (store / "pending").iterdir())`.
fn pending_suffixes(machine: &Machine) -> Vec<String> {
    let entries = std::fs::read_dir(machine.store().join("pending")).unwrap();
    let mut suffixes: Vec<String> = entries
        .map(|entry| {
            let name = entry.unwrap().file_name();
            let name = name.to_string_lossy();
            // The suffix starts at the last dot, unless that dot begins the name.
            match name.rfind('.') {
                Some(dot) if dot > 0 => name[dot..].to_owned(),
                _ => String::new(),
            }
        })
        .collect();
    suffixes.sort();
    suffixes
}

/// V6: after a lost acknowledgement, descriptor exhaustion must leave an attempt pending or
/// confirm it if the remote holds it; never discard it. Exercise `show`, `claim` and `sync` at
/// limits 8, 10, 11, 12 and 13. Measurements on Linux and macOS found reads possible from 10
/// descriptors and coordination from 11; the lower limits cover earlier failures. After a later
/// command succeeds, a retry is answered against the current state, where the issue is closed.
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
