//! The growth guards only time can keep (X15): ignored by every other run, they run nightly, in
//! release and one at a time, or not at all.

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
