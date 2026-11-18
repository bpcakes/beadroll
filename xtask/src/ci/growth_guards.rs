//! The growth guards only time can keep (X15): ignored by every other run, they run nightly, in
//! release and one at a time, or not at all; and core's differential tests over many seeds,
//! ignored too, which run nightly in release as well.

use super::workflow;

#[test]
fn the_nightly_times_the_growth_guards_in_release_one_at_a_time() {
    let nightly = workflow(".github/workflows/rust-nightly.yml");
    let step = nightly
        .lines()
        .find(|line| line.contains("test(/^timing::growth::/)"))
        .expect("the nightly runs no growth guard");
    for part in [
        "--release",
        "--run-ignored only",
        "--test-threads 1",
        "--package xtask",
    ] {
        assert!(
            step.contains(part),
            "the growth guards run without `{part}`: {step}"
        );
    }
}

/// The projection's differential tests over 10,000 seeds, which `docs/REVIEWING.md` cites as
/// evidence, are ignored by every run but the nightly's: in release, or they would never end.
#[test]
fn the_nightly_runs_cores_many_seeds_in_release() {
    let nightly = workflow(".github/workflows/rust-nightly.yml");
    let step = nightly
        .lines()
        .find(|line| line.contains("many_seeds_past_integrity_entries"))
        .expect("the nightly runs core's many seeds nowhere");
    for part in ["--release", "--run-ignored only", "--package beadroll-core"] {
        assert!(
            step.contains(part),
            "core's many seeds run without `{part}`: {step}"
        );
    }
}
