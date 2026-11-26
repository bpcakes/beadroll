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
    fn order(&self) -> JournalOrder {
        if self.sequenced {
            JournalOrder::Sequenced
        } else {
            JournalOrder::Attempt
        }
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

    fn read(&mut self, add: impl FnOnce(&mut Journal) -> bool) {
        if let Some(batch) = &mut self.readable
            && !add(&mut batch.0)
        {
            self.readable = None;
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
        self.read(|journal| journal.try_add_segment(data, replica, origin));
    }

    /// Adds the events of `data`, an attempt file found in `replica`'s journal at `origin`, as a
    /// journal reads one ([`Journal::add_attempt`]).
    pub fn add_attempt(&mut self, data: &[u8], replica: &str, origin: &str) {
        self.read(|journal| journal.try_add_attempt(data, replica, origin).is_ok());
    }

    /// Adds a record built in memory, with the order its file gives it (a journal's records
    /// sequenced, an attempt file's not). Catch-up requires a value that journal JSON reads
    /// back exactly. An attempt record must also pass its envelope and payload checks here.
    pub fn add(&mut self, record: Record) {
        if !self.is_readable() {
            record.value.discard();
            return;
        }
        self.read(|journal| {
            let order = record.order();
            journal.try_add_record(record.value, &record.replica, &record.origin, order)
        });
    }

    /// Whether nothing added was found unreadable so far: a line that is not JSON, an attempt line
    /// that is no valid event, a record with no readable id. A projection declines a batch with
    /// one, as it does one whose events fail as the journal settles them.
    pub fn is_readable(&self) -> bool {
        self.readable.is_some()
    }

    /// Reads through the first unreadable record, leaving the caller's iterator at the
    /// next one. For a vector of caller-built values, [`OwnedRecords`] can keep that
    /// suffix and dispose of it at any depth. An owned vector can go directly to `from`.
    ///
    /// ```compile_fail,E0308
    /// use beadroll_core::projection::{Appended, Record};
    /// fn collect_lazy(records: impl Iterator<Item = Record>) {
    ///     Appended::read_records(records);
    /// }
    /// ```
    pub fn read_records(records: &mut (impl Iterator<Item = Record> + ?Sized)) -> Self {
        let mut batch = Self::new();
        for record in records {
            batch.add(record);
            if !batch.is_readable() {
                break;
            }
        }
        batch
    }
}

/// A vector of caller-built records with an unread suffix. Inspection borrows the
/// records. Dropping the buffer releases even over-depth values without recursion.
pub struct OwnedRecords(std::vec::IntoIter<Record>);

impl OwnedRecords {
    /// Reads through the first unreadable record. The rest stay in this buffer.
    pub fn read_batch(&mut self) -> Appended {
        Appended::read_records(&mut self.0)
    }

    /// Records still available after the last batch, in their original order.
    pub fn remaining(&self) -> &[Record] {
        self.0.as_slice()
    }
}

impl From<Vec<Record>> for OwnedRecords {
    fn from(records: Vec<Record>) -> Self {
        Self(records.into_iter())
    }
}

impl Drop for OwnedRecords {
    fn drop(&mut self) {
        for record in &mut self.0 {
            record.value.discard();
        }
    }
}

impl From<OwnedRecords> for Appended {
    fn from(mut records: OwnedRecords) -> Self {
        records.read_batch()
    }
}

impl From<Vec<Record>> for Appended {
    fn from(records: Vec<Record>) -> Appended {
        OwnedRecords::from(records).into()
    }
}

/// A journal of `records`, each taken in with the order its file gives it, not yet settled: what a
/// test's history is read into.
#[cfg(test)]
pub(crate) fn journal_of(records: impl IntoIterator<Item = Record>) -> Journal {
    let mut journal = Journal::new();
    for record in records {
        let order = record.order();
        journal.add(record.value, &record.replica, &record.origin, order);
    }
    journal
}
