//! Readiness judged once for a whole state (roll-lqjj.19): which issues something holds, by the
//! rule [`State::open_blockers`] lists blockers by, without listing them. `bead ready`, `bead
//! blocked` and `bead prime` ask that of every issue, and asking it one issue at a time walked each
//! issue's ancestors in turn, so a chain of N issues cost N² steps; here every issue and every edge
//! is visited once. The question for one issue (`bead show`) is still answered on its own, by the
//! first thing found.

use std::collections::HashSet;

use crate::id::EventId;
use crate::state::{Issue, State};

use super::blocking;

/// Whether anything holds each issue of a state, judged once for all of them by the rule
/// [`State::open_blockers`] lists blockers by. [`State::readiness`] makes it, and the queries that
/// ask of many issues at once ([`State::ready`], `bead blocked`, `bead prime`) ask it rather than
/// walking each issue's ancestors in turn.
#[derive(Clone, Debug)]
pub struct Readiness<'a> {
    state: &'a State,
    /// Whether anything holds each issue, in the state's issue order.
    held: Vec<bool>,
}

impl Readiness<'_> {
    /// Whether anything holds `issue`: the answer [`State::is_blocked`] gives, judged for every
    /// issue at once. An issue that is not the state's is judged on its own.
    pub fn is_blocked(&self, issue: &Issue) -> bool {
        match self.state.issues.get_index_of(&issue.uid()) {
            Some(at) => self.held.get(at).copied().unwrap_or(false),
            None => self.state.is_blocked(issue),
        }
    }
}

impl State {
    /// Whether anything holds each issue, judged once for all of them (roll-lqjj.19), as
    /// [`State::open_blockers`] would list for each: the issues that are not done and have a
    /// prerequisite of their own are found first, then every issue that is not done and reaches one
    /// of them through parents that are not done, by one walk down the parent-child index from
    /// them; an issue is held when it has a prerequisite of its own, is an epic with an unfinished
    /// child, or has a parent so reached. Each issue, each edge and each child is visited once, a
    /// merged cycle once too, and nothing recurses, so the work grows with the issues and their
    /// edges rather than with the depth of every issue's ancestry.
    pub fn readiness(&self) -> Readiness<'_> {
        let at = |uid: EventId| self.issues.get_index_of(&uid);
        let named = |to: &str| EventId::parse(to).and_then(at);
        let flag = |flags: &[bool], at: usize| flags.get(at).copied().unwrap_or(false);
        let done: Vec<bool> = self.issues().map(|issue| self.is_done(issue)).collect();
        // A prerequisite of its own: a `blocks` target that is an issue and not done.
        let own: Vec<bool> = (self.issues())
            .map(|issue| {
                (issue.deps().keys()).any(|(to, kind)| {
                    blocking(kind) && named(to).is_some_and(|target| !flag(&done, target))
                })
            })
            .collect();
        // Down the parent-child index from every issue that is not done and has a prerequisite of
        // its own, through children that are not done: each child so reached would find that
        // prerequisite by walking up its parents, and so would everything beneath it.
        let mut reached = vec![false; done.len()];
        let mut pending: Vec<usize> = (0..done.len())
            .filter(|&at| !flag(&done, at) && flag(&own, at))
            .collect();
        for &at in &pending {
            if let Some(mark) = reached.get_mut(at) {
                *mark = true;
            }
        }
        while let Some(at) = pending.pop() {
            let Some((uid, _)) = self.issues.get_index(at) else {
                continue;
            };
            for child in self.children(*uid) {
                let Some(child) = self.issues.get_index_of(child) else {
                    continue;
                };
                if flag(&done, child) || flag(&reached, child) {
                    continue;
                }
                if let Some(mark) = reached.get_mut(child) {
                    *mark = true;
                }
                pending.push(child);
            }
        }
        let held = (self.issues().enumerate())
            .map(|(at, issue)| {
                flag(&own, at)
                    || issue.issue_type() == "epic"
                        && (self.children(issue.uid()).iter()).any(|child| {
                            self.issues
                                .get_index_of(child)
                                .is_some_and(|child| !flag(&done, child))
                        })
                    || (issue.deps().keys()).any(|(to, kind)| {
                        kind == "parent-child"
                            && named(to).is_some_and(|parent| flag(&reached, parent))
                    })
            })
            .collect();
        Readiness { state: self, held }
    }

    /// Whether anything holds `issue`: the question [`State::open_blockers`] answers by listing,
    /// answered by the first thing found, for one issue at a time (`bead show`). It walks `issue`'s
    /// ancestors, so a deep chain asked issue by issue costs the square of its depth; the queries
    /// that ask of many issues at once judge them all in one pass with [`State::readiness`].
    pub fn is_blocked(&self, issue: &Issue) -> bool {
        if self.holds_directly(issue) || self.waits_for_children(issue) {
            return true;
        }
        let mut seen = HashSet::from([issue.uid()]);
        let mut pending: Vec<&Issue> = self.parent_issues(issue).collect();
        while let Some(parent) = pending.pop() {
            if !seen.insert(parent.uid()) || self.is_done(parent) {
                continue;
            }
            if self.holds_directly(parent) {
                return true;
            }
            pending.extend(self.parent_issues(parent));
        }
        false
    }

    /// Whether `issue` has a prerequisite of its own: a `blocks` target that is an issue and not
    /// done, as [`State::direct_blockers`] lists them.
    fn holds_directly(&self, issue: &Issue) -> bool {
        (issue.deps().keys()).any(|(to, kind)| {
            blocking(kind)
                && self
                    .issue_named(to)
                    .is_some_and(|target| !self.is_done(target))
        })
    }

    /// Whether `issue` is an epic with a child that is not done: only an epic waits for its
    /// children, and nothing beneath it inherits that wait.
    fn waits_for_children(&self, issue: &Issue) -> bool {
        issue.issue_type() == "epic"
            && (self.children(issue.uid()).iter())
                .any(|child| self.issue(*child).is_some_and(|child| !self.is_done(child)))
    }

    /// The issues `issue` names as parents, in the order its edges were added: what
    /// [`State::parents_of`] sorts, for a walk whose answer does not depend on the order.
    fn parent_issues<'s>(&'s self, issue: &'s Issue) -> impl Iterator<Item = &'s Issue> {
        (issue.deps().keys())
            .filter(|(_, kind)| kind == "parent-child")
            .filter_map(|(to, _)| self.issue_named(to))
    }
}
