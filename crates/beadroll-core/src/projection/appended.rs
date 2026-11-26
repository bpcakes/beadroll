//! The records a projection takes in ([`Projection::prepare_appended`]): a journal of them alone,
//! not yet settled. A store reads them from the bytes its journal files grew by, each line straight
//! into what the journal keeps of it, as it loads a whole journal (roll-lqjj.53.1); a transaction's
//! own, and a test's, are [`Record`]s built in memory, each read as the line bead writes of it
//! would be ([`Journal::add`]).

use crate::id::EventId;
use crate::journal::{Event, Journal, JournalOrder};
use crate::json::Value;
use crate::op::Op;

use super::Projection;

/// A record of a journal's files, to apply: an event as it was read, the machine whose folder held
/// it, the file, and whether it is one of that machine's journal's records (an attempt file's are
/// not).
#[derive(Clone, Debug)]
pub struct Record {
    /// The event, as read.
    pub value: Value,
    /// The machine whose folder held it.
    pub replica: String,
    /// The file it was read from.
    pub origin: String,
    /// Whether it is a record of that machine's journal.
    pub sequenced: bool,
}

impl Record {
    fn add_to(self, journal: &mut Journal) {
        let order = if self.sequenced {
            JournalOrder::Sequenced
        } else {
            JournalOrder::Attempt
        };
        journal.add(self.value, &self.replica, &self.origin, order);
    }
}

/// The records appended after a projection's journal, in the order read. A read error
/// discards the accumulated records and makes subsequent additions to this batch inert.
#[derive(Clone, Debug)]
pub struct Appended {
    // A read error rules out this and all later records in the same batch.
    readable: Option<ReadableAppended>,
}

/// A batch with no recorded read errors. Consuming the builder freezes the journal for
/// text selection and then application.
#[derive(Clone, Debug)]
pub(super) struct ReadableAppended(Journal);

impl ReadableAppended {
    pub(super) fn records(&self) -> &[Event] {
        self.0.raw()
    }

    pub(super) fn into_journal(self) -> Journal {
        self.0
    }

    /// The issues named by the records and those whose imported dependencies a create
    /// could bind. Applying that create also needs the waiting issues' edge metadata.
    pub(super) fn about(&self, projection: &Projection) -> Vec<EventId> {
        let mut about = Vec::new();
        for event in self.records() {
            about.extend(event.issue().and_then(EventId::parse));
            if let Op::Create(making) = event.op()
                && !projection.state.by_key.contains_key(&making.key)
            {
                about.extend(projection.state.awaiting.waiting(&making.key));
            }
        }
        about
    }
}

impl Default for Appended {
    fn default() -> Self {
        Self {
            readable: Some(ReadableAppended(Journal::new())),
        }
    }
}

impl Appended {
    /// Whether the batch contains neither records nor read errors.
    pub(crate) fn is_empty(&self) -> bool {
        self.readable()
            .is_some_and(|batch| batch.records().is_empty())
    }

    pub(super) fn readable(&self) -> Option<&ReadableAppended> {
        self.readable.as_ref()
    }

    pub(super) fn into_readable(self) -> Option<ReadableAppended> {
        self.readable
    }

    fn read(&mut self, add: impl FnOnce(&mut Journal)) {
        if let Some(batch) = &mut self.readable {
            add(&mut batch.0);
            if !batch.0.invalid.is_empty() {
                self.readable = None;
            }
        }
    }

    /// No records.
    pub fn new() -> Appended {
        Appended::default()
    }

    /// Adds the records of `data`, the bytes a segment found in `replica`'s journal at `origin`
    /// grew by, as a journal reads a segment ([`Journal::add_segment`]). Bytes that end in a line
    /// not yet whole are unreadable here: a journal skips such a line as one still being appended,
    /// but a batch that skipped it would be applied without it, and its reader would go on after
    /// bytes the projection never took in.
    pub fn add_segment(&mut self, data: &[u8], replica: &str, origin: &str) {
        if data.last().is_some_and(|last| *last != b'\n') {
            self.readable = None;
            return;
        }
        self.read(|journal| journal.add_segment(data, replica, origin));
    }

    /// Adds the events of `data`, an attempt file found in `replica`'s journal at `origin`, as a
    /// journal reads one ([`Journal::add_attempt`]).
    pub fn add_attempt(&mut self, data: &[u8], replica: &str, origin: &str) {
        self.read(|journal| journal.add_attempt(data, replica, origin));
    }

    /// Adds a record built in memory, with the order its file gives it (a journal's records
    /// sequenced, an attempt file's not).
    pub fn add(&mut self, record: Record) {
        self.read(|journal| record.add_to(journal));
    }

    /// Whether nothing added was found unreadable so far: a line that is not JSON, an attempt line
    /// that is no valid event, a record with no readable id. A projection declines a batch with
    /// one, as it does one whose events fail as the journal settles them.
    pub fn is_readable(&self) -> bool {
        self.readable.is_some()
    }
}

impl FromIterator<Record> for Appended {
    fn from_iter<I: IntoIterator<Item = Record>>(records: I) -> Appended {
        let mut appended = Appended::new();
        for record in records {
            appended.add(record);
        }
        appended
    }
}

impl From<Vec<Record>> for Appended {
    fn from(records: Vec<Record>) -> Appended {
        records.into_iter().collect()
    }
}

/// A journal of `records`, each taken in with the order its file gives it, not yet settled: what a
/// test's history is read into.
#[cfg(test)]
pub(crate) fn journal_of(records: impl IntoIterator<Item = Record>) -> Journal {
    let mut journal = Journal::new();
    for record in records {
        record.add_to(&mut journal);
    }
    journal
}
