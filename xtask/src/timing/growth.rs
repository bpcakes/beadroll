//! The growth only time can guard (roll-lqjj.34, X15). beadroll-core's `work` guards count the work
//! its algorithms do where every way of doing it passes, and fail when it grows with the square of
//! the input; a count cannot see work done some other way, such as a lookup turned back into a scan
//! of a vector, or a merge into a vector that moves its tail. These time the core calls whose work
//! that can hide in — planning an import, validating it for a dry run, re-importing an issue whose
//! dependencies wait, replaying imports that wait for keys, replaying one issue given an edge on
//! every other and the same edges removed, by target and by key (roll-lqjj.21), replaying copies
//! two machines minted alike, which are renamed, of plain keys and of numbered ones (roll-lqjj.46),
//! and a projection taking ids that sort before every id it holds — on the shapes their `timing`
//! probes measure, at a size and at [`LARGER`] times it, and fail when the larger takes more than
//! [`BOUND`] times as long.
//!
//! The one exception to "timings never gate" (`xtask/AGENTS.md`): each test is ignored, because a
//! debug build's figures say nothing and a loaded machine's little more, and the nightly workflow
//! runs them in release on a runner of its own, `cargo nextest run --release --locked --package
//! xtask --run-ignored only -E 'test(/^timing::growth::/)'`. The fastest of [`RUNS`] runs of each
//! size counts, the sizes run in turn so that they share the machine's conditions.

use std::time::Instant;

use beadroll_core::import::dry_run::problems;
use beadroll_core::import::{plan_import, read_records};
use beadroll_core::replay::replay;

use super::{dangling, extend, plan, renames};

/// How many times each size is timed; the fastest run counts.
const RUNS: usize = 5;

/// How many times larger the larger input is.
const LARGER: usize = 8;

/// How many times as long as the smaller input the larger may take: three times what work growing
/// with its input takes ([`LARGER`]), where work growing with its square takes [`LARGER`]², 64.
const BOUND: f64 = 24.0;

/// Asserts that `call` on what `build` makes of `small` and of [`LARGER`] times it takes time that
/// grows with the input: at most [`BOUND`] times as long on the larger. Only the call is timed;
/// what it returns is dropped after.
fn grows_linearly<T, U>(
    what: &str,
    small: usize,
    build: impl Fn(usize) -> T,
    call: impl Fn(T) -> U,
) {
    let sizes = [small, LARGER * small];
    let mut best = [f64::INFINITY; 2];
    for _ in 0..RUNS {
        for (fastest, size) in best.iter_mut().zip(sizes) {
            let input = build(size);
            let clock = Instant::now();
            let answer = call(input);
            *fastest = fastest.min(clock.elapsed().as_secs_f64());
            drop(answer);
        }
    }
    let [small_seconds, large_seconds] = best;
    let ratio = large_seconds / small_seconds;
    println!(
        "{what}: {small_seconds:.4} s at {}, {large_seconds:.4} s at {}, {ratio:.1} times",
        sizes[0], sizes[1],
    );
    assert!(
        ratio <= BOUND,
        "{what}: {small_seconds:.4} s at {}, {large_seconds:.4} s at {}: {ratio:.1} times as long, \
         more than {BOUND}",
        sizes[0],
        sizes[1],
    );
}

#[test]
#[ignore = "a ratio of release timings: the nightly workflow runs it"]
fn planning_an_import_takes_time_growing_with_its_records() {
    grows_linearly(
        "planning an import whose records depend on its last",
        2_000,
        |records| {
            let parsed = read_records(&plan::export(records), plan::PATH).unwrap();
            (plan::tracker(0).unwrap(), parsed)
        },
        |(state, records)| plan_import(&state, &records).unwrap(),
    );
}

#[test]
#[ignore = "a ratio of release timings: the nightly workflow runs it"]
fn validating_an_import_for_a_dry_run_takes_time_growing_with_its_records() {
    grows_linearly(
        "validating that import for a dry run",
        4_000,
        |records| (plan::tracker(0).unwrap(), plan::export(records)),
        |(state, text)| problems(&state, &text, plan::PATH),
    );
}

#[test]
#[ignore = "a ratio of release timings: the nightly workflow runs it"]
fn re_importing_an_issue_whose_dependencies_wait_takes_time_growing_with_them() {
    grows_linearly(
        "re-importing one issue that still lists all its waiting dependencies",
        2_000,
        |waiting| {
            let again = read_records(&plan::reimport(waiting), plan::PATH).unwrap();
            (plan::tracker(waiting).unwrap(), again)
        },
        |(state, records)| plan_import(&state, &records).unwrap(),
    );
}

#[test]
#[ignore = "a ratio of release timings: the nightly workflow runs it"]
fn replaying_imports_that_wait_for_keys_takes_time_growing_with_them() {
    for shape in [dangling::Shape::Never, dangling::Shape::Arriving] {
        grows_linearly(
            &format!("replaying creates that wait for keys, {shape:?}"),
            2_000,
            |issues| dangling::journal(shape, issues),
            |journal| replay(journal).unwrap(),
        );
    }
}

#[test]
#[ignore = "a ratio of release timings: the nightly workflow runs it"]
fn replaying_one_issue_given_an_edge_on_every_other_takes_time_growing_with_them() {
    // an issue's edges are indexed once they are many (roll-lqjj.21); a count sees each edge looked
    // at, not whether a lookup reads every edge before it some other way
    for shape in [dangling::Shape::OneHasMany, dangling::Shape::OneAwaitsMany] {
        grows_linearly(
            &format!("replaying one issue given an edge on every other, {shape:?}"),
            4_000,
            |issues| dangling::journal(shape, issues),
            |journal| replay(journal).unwrap(),
        );
    }
    grows_linearly(
        "replaying one issue given an edge on every other by dep add, one event each",
        4_000,
        dangling::added,
        |journal| replay(journal).unwrap(),
    );
    grows_linearly(
        "replaying the same, and then each edge removed by dep remove, one event each",
        4_000,
        dangling::removed,
        |journal| replay(journal).unwrap(),
    );
    grows_linearly(
        "replaying an import naming every other issue's key, each then removed by its key",
        4_000,
        dangling::unbound,
        |journal| replay(journal).unwrap(),
    );
}

#[test]
#[ignore = "a ratio of release timings: the nightly workflow runs it"]
fn a_projection_taking_ids_before_every_id_it_holds_takes_time_growing_with_them() {
    grows_linearly(
        "closing every issue one event at a time, each id before every id held",
        4_000,
        |issues| {
            let head = extend::id(extend::created(issues));
            let closes = extend::closes_before_held(issues, &head);
            (extend::tracker(issues).unwrap(), closes)
        },
        |(projection, closes)| extend::one_at_a_time(projection, closes).unwrap(),
    );
}

#[test]
#[ignore = "a ratio of release timings: the nightly workflow runs it"]
fn replaying_copies_two_machines_minted_alike_takes_time_growing_with_them() {
    // renaming a copy asks of each key it tries whether an issue has it or an import waits for it
    // (roll-lqjj.25, X03), and a copy of a numbered key skips the runs of numbers earlier copies
    // found taken (roll-lqjj.46); a count sees the asks and the runs looked up, not whether one is
    // a lookup or a scan, which the copies with gaps, each looking its number up among N runs,
    // would show
    for shape in [
        renames::Shape::Renamed,
        renames::Shape::RenamedWaiting,
        renames::Shape::Numbered,
        renames::Shape::NumberedWaiting,
        renames::Shape::NumberedGaps,
    ] {
        grows_linearly(
            &format!("replaying two machines' creates of the same keys, {shape:?}"),
            2_000,
            |copies| renames::journal(shape, copies),
            |journal| replay(journal).unwrap(),
        );
    }
}
