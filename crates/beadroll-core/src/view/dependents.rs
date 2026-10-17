//! The issues that depend on each of several issues, found in one pass over every issue's edges
//! (roll-lqjj.20). A detail lists the issues that depend on its issue, and only the edges of every
//! issue say which those are; `bead show`, `bead update`, `bead claim` and `bead takeover` answer
//! with a detail for every issue they were given, so N issues read and sorted every edge N times.
//! Here every edge is read once for all of them, and only the edges that name one of them are kept
//! and sorted.

use std::collections::HashMap;

use crate::id::EventId;
use crate::state::{Issue, State, Target};

/// The issues that depend on each of some issues, found once for all of them:
/// [`State::dependents_of`] makes it, and [`State::detail_with`] lists an issue's dependents from
/// it.
#[derive(Clone, Debug)]
pub struct Dependents<'s> {
    /// The state they were found in: [`State::detail_with`] in any other finds its issue's own.
    state: &'s State,
    /// Each issue asked about, by its uid, which an edge to it names, and its place in `found`.
    asked: HashMap<EventId, usize>,
    /// For each issue asked about, every issue with an edge to it and that edge's type: the issues
    /// in replay order, each one's edges to it in sorted order.
    found: Vec<Vec<(&'s Issue, &'s str)>>,
}

impl<'s> Dependents<'s> {
    /// The issues with an edge to `issue`, each with the edge's type, when `issue` was asked about:
    /// what its detail lists as its dependents.
    pub fn of(&self, issue: &Issue) -> Option<&[(&'s Issue, &'s str)]> {
        let at = *self.asked.get(&issue.uid())?;
        self.found.get(at).map(Vec::as_slice)
    }

    /// Whether they were found in `state`.
    pub(crate) fn found_in(&self, state: &State) -> bool {
        std::ptr::eq(self.state, state)
    }

    /// How many distinct issues were asked about.
    pub(crate) fn asked(&self) -> usize {
        self.asked.len()
    }
}

impl State {
    /// The issues that depend on each of `issues`, found in one pass over every issue's edges for
    /// all of them (roll-lqjj.20), for [`State::detail_with`] to list: each issue with an edge to
    /// one of them, in replay order, once for each type of edge, the types in sorted order. An edge
    /// counts when it names an issue's uid, so an issue that took another's key is never confused
    /// with it, and an issue asked about twice is looked for once.
    pub fn dependents_of<'i>(&self, issues: impl IntoIterator<Item = &'i Issue>) -> Dependents<'_> {
        let mut asked: HashMap<EventId, usize> = HashMap::new();
        for issue in issues {
            let next = asked.len();
            asked.entry(issue.uid()).or_insert(next);
        }
        let mut found: Vec<Vec<(&Issue, &str)>> = vec![Vec::new(); asked.len()];
        if asked.is_empty() {
            return Dependents {
                state: self,
                asked,
                found,
            };
        }
        // One issue asked about, as `State::detail` asks, is looked for by comparing each edge's
        // target with its uid, which costs less than hashing the target.
        let only = match asked.len() {
            1 => asked.keys().next().copied(),
            _ => None,
        };
        let place = |to: &Target| {
            let to = to.id()?;
            match only {
                Some(uid) => (to == uid).then_some(0),
                None => asked.get(&to).copied(),
            }
        };
        // The edges of one issue that name an issue asked about, by its place and the edge's type:
        // sorted, they are in the order of its sorted edges for each issue they name.
        let mut edges: Vec<(usize, &str)> = Vec::new();
        for other in self.issues() {
            edges.clear();
            let named = (other.deps().keys())
                .filter_map(|edge| place(edge.to()).map(|at| (at, edge.kind())));
            edges.extend(named);
            edges.sort_unstable();
            for &(at, kind) in &edges {
                if let Some(listed) = found.get_mut(at) {
                    listed.push((other, kind));
                }
            }
        }
        Dependents {
            state: self,
            asked,
            found,
        }
    }
}
