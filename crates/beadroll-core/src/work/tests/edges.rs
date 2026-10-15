//! Readiness (roll-lqjj.19), the dependents of the issues shown in full (roll-lqjj.20) and what
//! the lists and details of several issues share (roll-lqjj.24): every issue's edges read a
//! bounded number of times, however deep its ancestry and however many issues are shown.

use crate::query::ReadyScope;
use crate::state::State;

use super::super::Work;
use super::{History, grows_linearly, key};

/// How many issues a row of the lattice holds.
const WIDTH: usize = 4;

/// How the issues readiness is judged on are linked: the shapes `timing --ready` measures.
#[derive(Clone, Copy, Debug)]
enum Shape {
    /// Issue 1 the prerequisite of issue 2, and each later issue the child of the one before.
    ChainHeld,
    /// The same chain, waiting for nothing.
    ChainFree,
    /// Issue 1 the prerequisite of the first row, and each later issue the child of two in the
    /// row above it.
    LatticeHeld,
}

impl Shape {
    const ALL: [Shape; 3] = [Shape::ChainHeld, Shape::ChainFree, Shape::LatticeHeld];

    /// What issue `n`, counting from one, depends on.
    fn linked(self, n: usize) -> Vec<(String, &'static str)> {
        match self {
            Shape::ChainHeld if n == 2 => vec![(key(1), "blocks")],
            Shape::ChainHeld | Shape::ChainFree if n > 2 => vec![(key(n - 1), "parent-child")],
            Shape::ChainHeld | Shape::ChainFree => Vec::new(),
            Shape::LatticeHeld if n == 1 => Vec::new(),
            Shape::LatticeHeld => {
                let (row, column) = ((n - 2) / WIDTH, (n - 2) % WIDTH);
                if row == 0 {
                    return vec![(key(1), "blocks")];
                }
                let above = |column: usize| key(2 + (row - 1) * WIDTH + column % WIDTH);
                vec![
                    (above(column), "parent-child"),
                    (above(column + 1), "parent-child"),
                ]
            }
        }
    }

    /// The state of `issues` issues so linked.
    fn state(self, issues: usize) -> State {
        let mut history = History::new();
        for n in 1..=issues {
            history.create(&key(n), &self.linked(n), &[]);
        }
        history.state()
    }
}

#[test]
fn judging_readiness_reads_each_edge_a_bounded_number_of_times() {
    for shape in Shape::ALL {
        grows_linearly(
            &format!("readiness of every issue of {shape:?}"),
            &[Work::Edge],
            |issues| shape.state(issues),
            |state| {
                let readiness = state.readiness();
                let held = state.issues().filter(|issue| readiness.is_blocked(issue));
                assert_eq!(held.count() == 0, matches!(shape, Shape::ChainFree));
            },
        );
        grows_linearly(
            &format!("the ready issues of {shape:?}"),
            &[Work::Edge],
            |issues| shape.state(issues),
            |state| drop(state.ready(ReadyScope::All)),
        );
    }
}

/// The state of `issues` issues on the graph `timing --details` measures, which has no
/// parent-child edge: issue n waits for issue n/2, and every third is related to the one before.
fn sparse(issues: usize) -> State {
    let mut history = History::new();
    for n in 1..=issues {
        let mut linked = Vec::new();
        if n > 1 {
            linked.push((key(n / 2), "blocks"));
        }
        if n > 1 && n % 3 == 0 {
            linked.push((key(n - 1), "related"));
        }
        history.create(&key(n), &linked, &[]);
    }
    history.state()
}

#[test]
fn the_details_of_every_issue_read_each_edge_a_bounded_number_of_times() {
    grows_linearly(
        "finding the dependents of every issue",
        &[Work::Edge],
        sparse,
        |state| {
            let dependents = state.dependents_of(state.issues());
            assert!(dependents.of(state.issues().next().unwrap()).is_some());
        },
    );
    grows_linearly(
        "the detail of every issue, its dependents found once",
        &[Work::Edge],
        sparse,
        |state| {
            let dependents = state.dependents_of(state.issues());
            for issue in state.issues() {
                drop(state.detail_with(issue, &dependents));
            }
        },
    );
}

#[test]
fn listing_the_blockers_of_every_held_issue_reads_each_edge_a_bounded_number_of_times() {
    // the shapes whose lists are linear in their issues: each held issue has one blocker, however
    // deep it lies, where a chain held by every issue above it would list N² on purpose
    for shape in [Shape::ChainHeld, Shape::LatticeHeld] {
        grows_linearly(
            &format!("the blockers of every issue of {shape:?}"),
            &[Work::Edge],
            |issues| shape.state(issues),
            |state| {
                let lists = state.blocker_lists();
                let held = state.issues().filter(|issue| !lists.of(issue).is_empty());
                assert_eq!(held.count(), state.issues().count() - 1);
            },
        );
    }
}

#[test]
fn the_details_of_every_issue_judge_readiness_reading_each_edge_a_bounded_number_of_times() {
    // every shape, the ones with parents too: a detail that walked its own ancestors for `blocked`
    // read N²/2 edges of a chain
    for shape in Shape::ALL {
        grows_linearly(
            &format!("the detail of every issue of {shape:?}, made together"),
            &[Work::Edge],
            |issues| shape.state(issues),
            |state| {
                let details = state.details_of(state.issues());
                for issue in state.issues() {
                    drop(details.of(issue));
                }
            },
        );
    }
    grows_linearly(
        "the detail of every issue of the sparse graph, made together",
        &[Work::Edge],
        sparse,
        |state| {
            let details = state.details_of(state.issues());
            for issue in state.issues() {
                drop(details.of(issue));
            }
        },
    );
}
