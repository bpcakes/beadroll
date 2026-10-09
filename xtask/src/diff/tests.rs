//! The differential's own tests: its workers.

use super::*;

#[test]
fn a_worker_that_panics_fails_the_run_instead_of_dropping_its_cases() {
    let items: Vec<u32> = (0..9).collect();
    let doubled = in_parallel(&items, 4, |n| n * 2).unwrap();
    assert_eq!(doubled, items.iter().map(|n| n * 2).collect::<Vec<_>>());
    // One worker of three panics: without the failure its three cases would go unanswered, and a
    // comparison that zips the answers would skip them.
    let one = in_parallel(&items, 3, |n| if *n == 4 { panic!("case {n}") } else { *n });
    assert!(one.is_err(), "{one:?}");
    // Every worker panics: still a failure, not a panic of the scope.
    let all = in_parallel(&items, 3, |n| -> u32 { panic!("case {n}") });
    assert!(all.is_err(), "{all:?}");
}
