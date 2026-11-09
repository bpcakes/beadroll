//! Borrowed evidence for reads a summary or a partially loaded state cannot answer.

use std::ops::Deref;

use crate::code::Code;
use crate::id::EventId;
use crate::json::Object;

use super::State;

/// Which event records a state knows about. This is independent of text hydration and
/// does not imply complete transitive ancestry for pending events ([`FullKnowledge::saw`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KnowledgeScope {
    /// Every event in the replayed journal, with replay's ancestry judgments.
    Full,
    /// The events a projection retains to apply later records, with their ancestry.
    Retained,
    /// Only the writers needed to describe conflicts; no complete ancestry.
    Summary,
}

/// An issue whose texts must be loaded before exporting the whole state.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("issue {issue} has texts that have not been loaded")]
pub struct MissingTexts {
    /// The first issue with texts still in a projection's text file.
    pub issue: EventId,
}

impl MissingTexts {
    /// A caller attempted a read without the required data.
    pub fn code(&self) -> Code {
        Code::Validation
    }
}

/// A state with every issue's texts loaded. Its shared borrow prevents unloading or
/// changing them while the capability is used. Obtain it through [`State::hydrated`].
///
/// ```compile_fail,E0624
/// use beadroll_core::state::State;
/// fn export_partial(state: &State) { state.export_records(); }
/// ```
#[derive(Clone, Copy, Debug)]
pub struct HydratedState<'a>(&'a State);

impl<'a> HydratedState<'a> {
    /// Replay constructs every issue with its texts present.
    pub(crate) fn replayed(state: &'a State) -> Self {
        Self(state)
    }

    /// Beads-compatible records with all texts, comments and dependency metadata.
    pub fn export_records(self) -> Vec<Object> {
        self.0.export_records()
    }
}

impl Deref for HydratedState<'_> {
    type Target = State;

    fn deref(&self) -> &State {
        self.0
    }
}

/// Every event from a full replay, never a projection's retained subset or a decoded
/// summary. Obtain it through [`State::full_knowledge`]. Membership, iteration and authorship
/// include pending events; their ancestry is limited as [`Self::saw`] describes.
///
/// ```compile_fail,E0624
/// use beadroll_core::{state::State, id::EventId};
/// fn ancestry(state: &State, a: EventId, b: EventId) { state.saw(a, b); }
/// ```
#[derive(Clone, Copy, Debug)]
pub struct FullKnowledge<'a>(&'a State);

impl<'a> FullKnowledge<'a> {
    pub(crate) fn replayed(state: &'a State) -> Self {
        Self(state)
    }

    /// Whether this replay's journal held the event, pending events included.
    pub fn holds(self, event: EventId) -> bool {
        self.0.holds(event)
    }

    /// Every event in the journal's installation order.
    pub fn events(self) -> impl Iterator<Item = EventId> + 'a {
        self.0.events()
    }

    /// Whether the writer of `event` had seen `other`.
    ///
    /// For an event in [`State::pending`], only explicitly named causes are recognized,
    /// even if a named cause is itself missing. A `false` answer does not rule out transitive
    /// ancestry: replay has not computed that event's clock. Once the missing causes arrive
    /// and a new replay places the event, its transitive ancestry is available too.
    pub fn saw(self, event: EventId, other: EventId) -> bool {
        self.0.saw(event, other)
    }

    /// Who wrote the event, or the established empty label for an unknown event.
    pub fn who(self, event: EventId) -> String {
        self.0.who(event)
    }
}

impl State {
    /// Whether event records cover the whole journal, a retained subset, or only conflict writers.
    pub fn knowledge_scope(&self) -> KnowledgeScope {
        self.knowledge_scope
    }

    /// Journal-wide event knowledge is available only from replay. Loading texts does not
    /// recover it. Pending ancestry still has the limits described by [`FullKnowledge::saw`].
    pub fn full_knowledge(&self) -> Option<FullKnowledge<'_>> {
        (self.knowledge_scope == KnowledgeScope::Full).then_some(FullKnowledge(self))
    }

    /// Borrows the state for complete export, or identifies texts the caller must load first.
    /// Summary and retained states may become hydrated without acquiring journal-wide knowledge.
    pub fn hydrated(&self) -> Result<HydratedState<'_>, MissingTexts> {
        match self.issues().find(|issue| !issue.texts().here()) {
            Some(issue) => Err(MissingTexts { issue: issue.uid() }),
            None => Ok(HydratedState(self)),
        }
    }
}
