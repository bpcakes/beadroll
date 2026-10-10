//! `Sweep.test_sweep`. Verified property (sweep): for every schedule, at most one confirmed owner,
//! generations monotonic, every confirmed claim one the remote accepted, identical exports, `show`
//! and `claims` everywhere after settling, no waiting events, integrity entries, pending attempts
//! or forked journals, and each command's answer consistent with what the remote holds. Measured
//! by the review: 660 schedules on each of three hosts, plus 660 with `git gc --prune=now` on the
//! remote after every push and 330 + 330 with the two follow-ups run concurrently; the only
//! failures were the ten G12 schedules, fixed since that review. `BEAD_SWEEP=full` runs all 660
//! (`BEAD_SWEEP_GC=1` and `BEAD_SWEEP_CONCURRENT=1` select the variants). Every selected schedule
//! must pass; each one that fails is named with its violations.

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};

use super::call::{Step, tail};
use super::run::Run;
use super::{SLICE, Schedule, Variant, schedules};

/// What one schedule came to: its violations, and its steps but the setup and the views.
pub(super) struct Report {
    pub(super) schedule: Schedule,
    pub(super) violations: Vec<String>,
    pub(super) steps: Vec<Step>,
}

/// Runs one schedule in a sandbox of its own, removed when it is done. A schedule that cannot even
/// be driven is a result too: a driver error.
fn run_schedule(schedule: Schedule, variant: Variant) -> Report {
    let mut run = match catch_unwind(|| Run::new(schedule, variant)) {
        Ok(run) => run,
        Err(panic) => {
            return Report {
                schedule,
                violations: vec![driver_error(panic.as_ref())],
                steps: Vec::new(),
            };
        }
    };
    if let Err(panic) = catch_unwind(AssertUnwindSafe(|| run.execute())) {
        run.violation(driver_error(panic.as_ref()));
    }
    let (violations, steps) = run.report();
    Report {
        schedule,
        violations,
        steps,
    }
}

/// A driver error, as Python records `driver error: {error!r}`: what the driver's panic said.
fn driver_error(panic: &(dyn Any + Send)) -> String {
    let said = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&str>().copied())
        .unwrap_or("a panic without a message");
    format!("driver error: {said}")
}

/// Runs `chosen` through `run` in parallel, on as many workers as the machine has cores (each
/// schedule in a sandbox of its own); the reports in the order of `chosen`.
fn run_all(chosen: &[Schedule], run: &(dyn Fn(Schedule) -> Report + Sync)) -> Vec<Report> {
    let cores = std::thread::available_parallelism().map_or(4, usize::from);
    let workers = cores.min(chosen.len()).max(1);
    let next = AtomicUsize::new(0);
    let reports = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                while let Some(schedule) = chosen.get(next.fetch_add(1, Ordering::Relaxed)) {
                    let report = run(*schedule);
                    let mut reports = reports.lock().unwrap_or_else(PoisonError::into_inner);
                    reports.push(report);
                }
            });
        }
    });
    let mut reports = reports.into_inner().unwrap_or_else(PoisonError::into_inner);
    let place = |report: &Report| chosen.iter().position(|s| *s == report.schedule);
    reports.sort_by_key(place);
    reports
}

/// A failed schedule, for the test's failure: the schedule, its violations, and its steps with
/// what each answered.
fn describe(report: &Report, variant: Variant) -> String {
    let Schedule { op, fault, pair } = report.schedule;
    let mut text = format!("{op} {fault} {pair} ({variant:?})\n  violations:\n");
    for violation in &report.violations {
        text.push_str(&format!("    - {violation}\n"));
    }
    text.push_str("  steps:\n");
    for step in &report.steps {
        let code = step.code.as_deref().unwrap_or("-");
        text.push_str(&format!(
            "    {} on {} as {}: bead {} -> rc {} {code} ({} ms)\n",
            step.step,
            step.machine,
            step.thread,
            step.argv.join(" "),
            step.rc,
            step.ms
        ));
        if !step.rc.is(0) && !step.err.trim().is_empty() {
            text.push_str(&format!("      stderr: {}\n", tail(step.err.trim(), 300)));
        }
    }
    text
}

/// Whether the variable `name` is set to `value`.
fn set(name: &str, value: &str) -> bool {
    std::env::var_os(name).is_some_and(|set| set == value)
}

/// The schedules a sweep runs: with `full` (`BEAD_SWEEP=full`) every one of `every`, else the
/// `slice`; in the concurrent variant only the pairs whose letters are in order, each unordered
/// pair once.
pub(super) fn selected(
    full: bool,
    concurrent: bool,
    every: Vec<Schedule>,
    slice: &[Schedule],
) -> Vec<Schedule> {
    let mut chosen = if full { every } else { slice.to_vec() };
    if concurrent {
        chosen.retain(|schedule| {
            let letters = schedule.pair.as_bytes();
            letters[0] < letters[1]
        });
    }
    chosen
}

/// Runs `chosen` through `run` and fails naming every schedule that found a violation, with its
/// violations and its steps.
pub(super) fn verdict(
    chosen: &[Schedule],
    variant: Variant,
    run: &(dyn Fn(Schedule) -> Report + Sync),
) -> Result<(), String> {
    let reports = run_all(chosen, run);
    let failures: Vec<String> = reports
        .iter()
        .filter(|report| !report.violations.is_empty())
        .map(|report| describe(report, variant))
        .collect();
    if failures.is_empty() {
        return Ok(());
    }
    Err(format!(
        "{} of {} schedules failed:\n\n{}",
        failures.len(),
        chosen.len(),
        failures.join("\n")
    ))
}

#[test]
fn sweep() {
    let every = schedules();
    assert_eq!(
        every.len(),
        660,
        "the sweep enumerates 55 cells of 12 pairs"
    );
    assert!(
        SLICE.iter().all(|slice| every.contains(slice)),
        "the slice is part of the sweep"
    );
    let variant = Variant {
        gc: set("BEAD_SWEEP_GC", "1"),
        concurrent: set("BEAD_SWEEP_CONCURRENT", "1"),
    };
    let chosen = selected(set("BEAD_SWEEP", "full"), variant.concurrent, every, &SLICE);
    if let Err(failures) = verdict(&chosen, variant, &|schedule| {
        run_schedule(schedule, variant)
    }) {
        panic!("{failures}");
    }
}
