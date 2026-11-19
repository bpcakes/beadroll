//! Loops a change to the dependency graph would close, judged before it is written: what `dep add`,
//! `update --parent` and `create` refuse as `CYCLE`. A child inherits what its parents pass down,
//! an edge [`State::waits_for`] does not hold, so a loop through it is no cycle `dep cycles` finds;
//! here an issue's inheritance is a node of its own, and the graph is walked as it will be after
//! the change, an old parent gone where `--parent` replaces it.

use std::collections::HashSet;

use crate::event::PARENT_CHILD;
use crate::id::EventId;
use crate::query::blocking;
use crate::state::State;

impl State {
    /// Whether `change`, about to be written, would have an issue wait for itself, judged on the
    /// graph as it will be after it: what `dep add`, `update --parent` and `create` refuse as
    /// `CYCLE`. An issue waits for its `blocks` targets, for its children (br's rule, as
    /// [`State::waits_for`] follows it), and for what each of its parents passes down, every
    /// `blocks` target of that parent and of the parent's own ancestors. That last is no edge of
    /// [`State::waits_for`], so that a loop through it is one `dep cycles` does not report; here
    /// each issue's inheritance is a node of its own ([`Waiting::Passed`]), so that the walk stays
    /// linear in the graph however deep it is. Done issues are judged too: reopening one would
    /// close the loop again.
    pub fn would_wait_for_itself(&self, change: GraphChange) -> bool {
        let graph = Planned { st: self, change };
        let starts: Vec<Waiting> = match change {
            // nothing new to close a loop: one the graph holds already is `dep cycles`' to report
            GraphChange::Unchanged => return false,
            GraphChange::Prerequisite { issue, target } if issue == target => return true,
            GraphChange::Parent { child, parent, .. } if child == parent => return true,
            // the new edges leave these: a new loop holds one of them
            GraphChange::Prerequisite { issue, .. } => {
                vec![Waiting::Issue(issue), Waiting::Passed(issue)]
            }
            GraphChange::Parent { child, parent, .. } => vec![
                Waiting::Issue(parent),
                Waiting::Issue(child),
                Waiting::Passed(child),
            ],
        };
        starts.into_iter().any(|start| graph.reaches(start, start))
    }

    /// Whether what `parent` passes down to a child (its `blocks` targets and its own ancestors')
    /// reaches `goal`: a new issue under both `parent` and `goal` waits for that, and `goal` waits
    /// for the new issue, its child, so that the two parents together close a loop.
    pub fn passes_down_to(&self, parent: EventId, goal: EventId) -> bool {
        let graph = Planned {
            st: self,
            change: GraphChange::Unchanged,
        };
        graph.reaches(Waiting::Passed(parent), Waiting::Issue(goal))
    }

    /// Whether `start` waits, directly or not, for `goal`, what its parents pass down counted
    /// ([`State::would_wait_for_itself`]): what `create` asks of a new issue's prerequisites
    /// against its parents, the only ones a new issue is reached through.
    pub fn waits_on(&self, start: EventId, goal: EventId) -> bool {
        let graph = Planned {
            st: self,
            change: GraphChange::Unchanged,
        };
        start == goal || graph.reaches(Waiting::Issue(start), Waiting::Issue(goal))
    }
}

/// A change to the dependency graph a command is about to write, as
/// [`State::would_wait_for_itself`] judges it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphChange {
    /// Nothing: the graph as it is.
    Unchanged,
    /// `issue` depends on `target` with a blocking dependency (`dep add`).
    Prerequisite {
        /// The issue that would wait.
        issue: EventId,
        /// What it would wait for.
        target: EventId,
    },
    /// `parent` becomes a parent of `child`, beside the parents it has (`dep add --type
    /// parent-child`) or in their place (`update --parent`).
    Parent {
        /// The issue given a parent.
        child: EventId,
        /// Its new parent.
        parent: EventId,
        /// Whether its other parents stay.
        others: OtherParents,
    },
}

/// What becomes of an issue's other parents when it is given one ([`GraphChange::Parent`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OtherParents {
    /// They stay: a parent added beside them.
    Kept,
    /// They go: the new parent is its only one.
    Replaced,
}

/// A node of the graph [`State::would_wait_for_itself`] walks: an issue, or what an issue passes
/// down to its children (its `blocks` targets and what its own parents pass down).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Waiting {
    Issue(EventId),
    Passed(EventId),
}

/// `st` as it will be after `change`, walked as waiting.
struct Planned<'s> {
    st: &'s State,
    change: GraphChange,
}

impl Planned<'_> {
    /// Whether `goal` is reached from `start` by one edge or more.
    fn reaches(&self, start: Waiting, goal: Waiting) -> bool {
        let mut seen = HashSet::new();
        let mut stack = self.next(start);
        while let Some(node) = stack.pop() {
            if node == goal {
                return true;
            }
            if seen.insert(node) {
                stack.extend(self.next(node));
            }
        }
        false
    }

    /// What `node` waits for, the change applied.
    fn next(&self, node: Waiting) -> Vec<Waiting> {
        let id = match node {
            Waiting::Issue(id) | Waiting::Passed(id) => id,
        };
        let mut out: Vec<Waiting> = self
            .prerequisites(id)
            .into_iter()
            .map(Waiting::Issue)
            .collect();
        match node {
            Waiting::Issue(_) => {
                out.extend(self.children(id).into_iter().map(Waiting::Issue));
                out.extend(self.parents(id).into_iter().map(Waiting::Passed));
            }
            Waiting::Passed(_) => out.extend(self.parents(id).into_iter().map(Waiting::Passed)),
        }
        out
    }

    /// `id`'s blocking targets that are issues, the change's among them.
    fn prerequisites(&self, id: EventId) -> Vec<EventId> {
        let mut out: Vec<EventId> = (self.st.issues.get(&id).into_iter())
            .flat_map(|issue| issue.deps().keys())
            .filter(|edge| blocking(edge.kind()))
            .filter_map(|edge| edge.to().id().filter(|to| self.st.issues.contains_key(to)))
            .collect();
        if let GraphChange::Prerequisite { issue, target } = self.change
            && issue == id
        {
            out.push(target);
        }
        out
    }

    /// `id`'s parents, the change applied.
    fn parents(&self, id: EventId) -> Vec<EventId> {
        let replaced = matches!(self.change, GraphChange::Parent { child, others: OtherParents::Replaced, .. } if child == id);
        let mut out: Vec<EventId> = if replaced {
            Vec::new()
        } else {
            (self.st.issues.get(&id).into_iter())
                .flat_map(|issue| issue.deps().keys())
                .filter(|edge| edge.kind() == PARENT_CHILD)
                .filter_map(|edge| edge.to().id().filter(|to| self.st.issues.contains_key(to)))
                .collect()
        };
        if let GraphChange::Parent { child, parent, .. } = self.change
            && child == id
        {
            out.push(parent);
        }
        out
    }

    /// `id`'s children, the change applied.
    fn children(&self, id: EventId) -> Vec<EventId> {
        let mut out: Vec<EventId> = self.st.children(id).to_vec();
        if let GraphChange::Parent {
            child,
            parent,
            others,
        } = self.change
        {
            if others == OtherParents::Replaced && parent != id {
                out.retain(|c| *c != child);
            }
            if parent == id && !out.contains(&child) {
                out.push(child);
            }
        }
        out
    }
}
