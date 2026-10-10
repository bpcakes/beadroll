//! `test_check.SweepFailureReporting`, ported (`docs/RUST-PORT.md` §4.6): the sweep fails when a
//! schedule fails, in the slice and in the full sweep alike, and passes when none does; and its
//! slice keeps retrying the owner's close and delete interrupted before publishing (G12). Where
//! Python patched `schedules`, `SLICE` and `run_schedule`, a synthetic report stands in for the run
//! of a sweep whose every list is that one schedule. These tests live beside the sweep they check,
//! whose runner is test code of this binary that xtask cannot reach.

use super::sweep::{Report, selected, verdict};
use super::{SLICE, Schedule, Variant, schedule};

/// The pairs that retry first, or after another actor or machine: G12's interrupted closes and
/// deletes, retried.
const RETRIES: [&str; 5] = ["RD", "RO", "RS", "OR", "SR"];

/// The verdict of a sweep whose only schedule is `only`, reporting `injected failure` unless
/// `ok`: the full sweep's list with `full`, else the slice, as `BEAD_SWEEP` picks them.
fn sweep_of(only: Schedule, ok: bool, full: bool) -> Result<(), String> {
    let chosen = selected(full, false, vec![only], &[only]);
    let sequential = Variant {
        gc: false,
        concurrent: false,
    };
    let report = |schedule| Report {
        schedule,
        violations: if ok {
            Vec::new()
        } else {
            vec!["injected failure".to_owned()]
        },
        steps: Vec::new(),
    };
    verdict(&chosen, sequential, &report)
}

#[test]
fn every_failed_schedule_fails_the_sweep() {
    // The ten G12 cases used to be accepted even after the underlying bug was fixed.
    let mut failing: Vec<Schedule> = ["close", "delete"]
        .into_iter()
        .flat_map(|op| RETRIES.map(|pair| schedule(op, "crash-before-push", pair)))
        .collect();
    failing.push(schedule("claim", "crash-after-push", "RO"));
    for full in [false, true] {
        for only in &failing {
            let failed = sweep_of(*only, false, full);
            let message = failed.expect_err("a failed schedule fails the sweep");
            assert!(message.starts_with("1 of 1 schedules failed"), "{message}");
            assert!(message.contains("injected failure"), "{message}");
        }
    }
}

#[test]
fn successful_schedules_pass() {
    for full in [false, true] {
        let passing = schedule("close", "crash-before-push", "RS");
        assert_eq!(sweep_of(passing, true, full), Ok(()), "full: {full}");
    }
}

#[test]
fn default_slice_retries_crashed_owner_close_and_delete() {
    for op in ["close", "delete"] {
        let retried = SLICE.iter().any(|slice| {
            slice.op == op && slice.fault == "crash-before-push" && RETRIES.contains(&slice.pair)
        });
        assert!(
            retried,
            "the slice retries no {op} crashed before publishing"
        );
    }
}
