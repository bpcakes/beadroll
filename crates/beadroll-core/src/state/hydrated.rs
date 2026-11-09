//! Selective evidence that an issue's text-bearing answers are complete.

use std::ops::Deref;

use crate::event::Field;
use crate::json::{Object, Value};

use super::{Comment, EndedClaim, IdMap, Issue, LostClaim, MissingTexts, Stale, VecMap, is_text};

/// One issue with its long fields, comments, extras and ownership history loaded.
/// Obtain it through [`Issue::hydrated`]; other issues may remain unloaded.
/// The shared borrow keeps the evidence valid while it is used.
///
/// ```compile_fail,E0624
/// use beadroll_core::state::Issue;
/// fn comments(issue: &Issue) { issue.comments(); }
/// ```
///
/// ```compile_fail,E0308
/// use beadroll_core::state::{Issue, State};
/// fn detail(state: &State, issue: &Issue) { state.detail(issue); }
/// ```
#[derive(Clone, Copy, Debug)]
pub struct HydratedIssue<'a>(&'a Issue);

impl<'a> HydratedIssue<'a> {
    /// The underlying issue, for summary and topology queries.
    pub fn as_issue(self) -> &'a Issue {
        self.0
    }

    /// A field's value, including a create's explicit null, or `None` when absent.
    pub fn field(self, field: Field) -> Option<&'a Value> {
        self.0.field(field)
    }

    /// A field's value, or `None` when absent or null.
    pub fn get(self, field: Field) -> Option<&'a Value> {
        self.0.get(field)
    }

    /// Every value behind each field, including the long fields' registers.
    pub fn registers(self) -> &'a VecMap<Field, IdMap<Value>> {
        self.0.registers()
    }

    /// Imported comments followed by native comments in replay order.
    pub fn comments(self) -> &'a [Comment] {
        self.0.comments()
    }

    /// Imported fields bead does not know, and what a delete records.
    pub fn extra(self) -> &'a Object {
        self.0.extra()
    }

    /// Claims that ended, oldest first.
    pub fn claim_history(self) -> &'a [EndedClaim] {
        self.0.claim_history()
    }

    /// Claims that lost to the owner of the time.
    pub fn lost_claims(self) -> &'a [LostClaim] {
        self.0.lost_claims()
    }

    /// Ends of claims that were no longer current and were ignored.
    pub fn stale(self) -> &'a [Stale] {
        self.0.stale()
    }
}

impl Deref for HydratedIssue<'_> {
    type Target = Issue;

    fn deref(&self) -> &Issue {
        self.0
    }
}

impl Issue {
    /// Borrows this issue's complete text-bearing answers, or identifies the texts to load.
    /// This checks only this issue, without requiring the rest of its state to be hydrated.
    pub fn hydrated(&self) -> Result<HydratedIssue<'_>, MissingTexts> {
        if self.texts().here() {
            Ok(HydratedIssue(self))
        } else {
            Err(MissingTexts { issue: self.uid() })
        }
    }

    /// A field's value, including a create's explicit null, or `None` when absent.
    /// Short fields are always available; a long field whose texts are not loaded is an error,
    /// including when that field will turn out to be absent or empty.
    pub fn read_field(&self, field: Field) -> Result<Option<&Value>, MissingTexts> {
        if is_text(field) {
            self.hydrated()?;
        }
        Ok(self.field(field))
    }
}
