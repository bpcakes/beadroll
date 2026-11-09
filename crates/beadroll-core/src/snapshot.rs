//! One immutable journal read, its replayed state and the recap selections made from it.

use crate::id::EventId;
use crate::journal::{Event, Journal, SettledJournal};
use crate::recap::{self, Unseen, Window};
use crate::replay::replay;
use crate::state::{FullKnowledge, HydratedState, Issue, KeySpace, State};

/// A settled journal paired with its own full replay. There is no constructor accepting
/// a separate state, and none of its parts can be mutated through this interface.
///
/// A published-tree consumer can use [`crate::published::Tree::snapshot`] directly.
///
/// ```compile_fail,E0451
/// use beadroll_core::{journal::Journal, state::State, snapshot::Snapshot};
/// fn pair(journal: Journal, state: State) -> Snapshot { Snapshot { journal, state } }
/// ```
#[derive(Clone, Debug)]
pub struct Snapshot {
    journal: Journal,
    state: State,
}

impl Snapshot {
    /// Settles one read and replays that exact journal. Events are shared between the
    /// journal and its replay input, as when a consumer previously cloned the journal itself.
    pub fn from_journal(mut journal: Journal) -> Result<Self, KeySpace> {
        journal.settle();
        let state = replay(journal.clone())?;
        Ok(Self { journal, state })
    }

    /// This snapshot's immutable, settled journal.
    pub fn journal(&self) -> SettledJournal<'_> {
        SettledJournal::new(&self.journal)
    }

    /// The state replayed from this journal, with every text loaded.
    pub fn state(&self) -> HydratedState<'_> {
        HydratedState::replayed(&self.state)
    }

    /// The journal-wide event knowledge from this replay. Pending events are included,
    /// but their ancestry queries recognize only direct causes ([`FullKnowledge::saw`]).
    pub fn knowledge(&self) -> FullKnowledge<'_> {
        FullKnowledge::replayed(&self.state)
    }

    /// An event bound to the state that knows what this journal made of its issue.
    pub fn event(&self, id: EventId) -> Option<SnapshotEvent<'_>> {
        self.journal().event(id).map(|event| SnapshotEvent {
            snapshot: self,
            event,
        })
    }

    /// A selection and acknowledgement frontier bound to this snapshot. Computing it
    /// does not mark anything seen; the caller persists the frontier after delivery.
    pub fn recap(&self, window: Window<'_>) -> Recap<'_> {
        Recap {
            snapshot: self,
            unseen: recap::unseen(self.journal(), window),
        }
    }
}

/// An event and the snapshot that supplies its replayed issue and export.
#[derive(Clone, Copy, Debug)]
pub struct SnapshotEvent<'a> {
    snapshot: &'a Snapshot,
    event: &'a Event,
}

impl<'a> SnapshotEvent<'a> {
    /// The journal record, including its original operation and provenance.
    pub fn event(self) -> &'a Event {
        self.event
    }

    /// Its issue, only when the same snapshot's replay created it.
    pub fn issue(self) -> Option<&'a Issue> {
        self.event
            .issue()
            .and_then(|uid| self.snapshot.state.issue_named(uid))
    }

    /// The fully hydrated state of this event's snapshot.
    pub fn state(self) -> HydratedState<'a> {
        self.snapshot.state()
    }

    /// Whether the snapshot found conflicting contents for this event id.
    pub fn quarantined(self) -> bool {
        self.snapshot
            .journal()
            .quarantined()
            .contains_key(&self.event.id())
    }
}

/// Recap results tied to the journal and state used to compute them. The ID lists are
/// immutable; joining another frontier always uses this selection's own sorted placed set.
#[derive(Clone, Debug)]
pub struct Recap<'a> {
    snapshot: &'a Snapshot,
    unseen: Unseen,
}

impl<'a> Recap<'a> {
    /// The source snapshot, also supplying the export for this selection.
    pub fn snapshot(&self) -> &'a Snapshot {
        self.snapshot
    }

    /// Selected events in causal order, with their issue resolution bound to this snapshot.
    pub fn events(&self) -> impl Iterator<Item = SnapshotEvent<'a>> + '_ {
        self.unseen
            .events
            .iter()
            .filter_map(|id| self.snapshot.event(*id))
    }

    /// Every placed event, including init and events outside the window, in ID order.
    pub fn placed(&self) -> impl Iterator<Item = SnapshotEvent<'a>> + '_ {
        self.unseen
            .placed
            .iter()
            .filter_map(|id| self.snapshot.event(*id))
    }

    /// Events still waiting for causes, in journal installation order.
    pub fn pending(&self) -> impl Iterator<Item = SnapshotEvent<'a>> + '_ {
        self.snapshot
            .journal()
            .events()
            .keys()
            .filter(|id| self.unseen.placed.binary_search(id).is_err())
            .filter_map(|id| self.snapshot.event(*id))
    }

    /// Selected event IDs in causal order, init excluded.
    pub fn event_ids(&self) -> &[EventId] {
        &self.unseen.events
    }

    /// Sorted IDs of every event placed by this read.
    pub fn placed_ids(&self) -> &[EventId] {
        &self.unseen.placed
    }

    /// The frontier to persist once this selection has been delivered.
    pub fn frontier(&self) -> &[EventId] {
        &self.unseen.frontier
    }

    /// How many events a first time window left out, init excluded.
    pub fn earlier(&self) -> u64 {
        self.unseen.earlier
    }

    /// Joins a previously saved or concurrent frontier using this read's own placed set.
    pub fn joined(&self, other: &[EventId]) -> Vec<EventId> {
        recap::joined(self.frontier(), self.placed_ids(), other)
    }
}
