//! Sweep reporting must fail if any schedule fails, in the default slice or full sweep. The slice
//! includes retries of interrupted owner close/delete and a sync interrupted at push. A schedule
//! whose fault never fires also fails. Synthetic reports exercise reporting without running the
//! protocol. These tests live beside the sweep runner, which is test code inaccessible to xtask.

use super::call::Rc;
use super::sweep::{Report, selected, verdict};
use super::{FAULTS, SLICE, Schedule, Variant, fault_fired, schedule};

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

/// A schedule whose operation ended without reaching its boundary tests nothing (roll-dih8.10: the
/// sync schedules at the push and timeout boundaries returned 0 with nothing to push): only the
/// end its fault gives counts as the fault having fired.
#[test]
fn a_fault_that_never_fired_fails_its_schedule() {
    for fault in FAULTS {
        assert!(
            !fault_fired(fault, Rc::Exit(0), false),
            "{fault} after exit 0"
        );
        assert!(!fault_fired(fault, Rc::Hang, false), "{fault} after a hang");
    }
    assert!(fault_fired("crash-before-push", Rc::Exit(98), false));
    assert!(!fault_fired("crash-before-push", Rc::Exit(99), false));
    assert!(fault_fired("crash-after-push", Rc::Exit(99), false));
    assert!(!fault_fired("crash-after-push", Rc::Exit(98), false));
    for fault in ["kill-confirm", "kill-write-pending", "kill-write-confirm"] {
        assert!(fault_fired(fault, Rc::Exit(-9), false), "{fault}");
        assert!(!fault_fired(fault, Rc::Exit(1), false), "{fault}");
    }
    for fault in ["timeout-fenced", "timeout-landed"] {
        assert!(fault_fired(fault, Rc::Exit(1), true), "{fault}");
        assert!(fault_fired(fault, Rc::Exit(0), true), "{fault}");
    }
}

#[test]
fn the_slice_stops_a_sync_at_its_push() {
    let pushed = SLICE.iter().any(|slice| {
        slice.op == "sync" && matches!(slice.fault, "crash-before-push" | "crash-after-push")
    });
    assert!(pushed, "the slice stops no sync at its push");
}
