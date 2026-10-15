//! The details of several issues made together (roll-lqjj.24). Two members of a detail ask of the
//! whole state: its dependents, which only every issue's edges say (roll-lqjj.20), and whether
//! anything holds it, which [`State::is_blocked`] answers by walking the issue's ancestors, so that
//! `bead show` of every issue of a chain of N walked N²/2 ancestors. Here both are found once for
//! all of the issues, as `bead ready` judges readiness ([`State::readiness`]), and a detail of one
//! issue is still made as [`State::detail`] makes it.

use crate::json::Object;
use crate::query::Readiness;
use crate::state::{Issue, State};

use super::Dependents;

/// What the details of some issues share, found once for all of them: the issues that depend on
/// each ([`Dependents`]) and, when there are several, whether anything holds each issue
/// ([`Readiness`]). [`State::details_of`] makes it, and [`Details::of`] gives each detail.
#[derive(Clone, Debug)]
pub struct Details<'s> {
    state: &'s State,
    dependents: Dependents<'s>,
    /// Readiness judged for every issue, when several were asked about; for one, its detail walks
    /// its own ancestors, which costs less than judging every issue.
    readiness: Option<Readiness<'s>>,
}

impl Details<'_> {
    /// The detail of `issue`, as [`State::detail`] gives it: an issue it was not made for has its
    /// dependents found on its own, as [`State::detail_with`] finds them.
    pub fn of(&self, issue: &Issue) -> Object {
        let blocked = match &self.readiness {
            Some(readiness) => readiness.is_blocked(issue),
            None => self.state.is_blocked(issue),
        };
        (self.state).detail_judged(issue, &self.dependents, blocked)
    }
}

impl State {
    /// What the details of `issues` share, found once for all of them (roll-lqjj.24): their
    /// dependents ([`State::dependents_of`]) and, for several distinct issues, every issue's
    /// readiness ([`State::readiness`]), which the issues' dependents already cost a pass over every
    /// edge for. `bead show`, `bead update`, `bead claim` and `bead takeover` make their answers'
    /// details from it.
    pub fn details_of<'i>(&self, issues: impl IntoIterator<Item = &'i Issue>) -> Details<'_> {
        let dependents = self.dependents_of(issues);
        let readiness = (dependents.asked() > 1).then(|| self.readiness());
        Details {
            state: self,
            dependents,
            readiness,
        }
    }
}
