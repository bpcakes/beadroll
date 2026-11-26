//! The records a projection takes in ([`Projection::prepare_appended`]): a journal of them alone,
//! not yet settled. A store reads them from the bytes its journal files grew by, each line straight
//! into what the journal keeps of it, as it loads a whole journal (roll-lqjj.53.1); a transaction's
//! own, and a test's, are [`Record`]s built in memory, each read as the line bead writes of it
//! would be ([`Journal::add`]).

use crate::journal::{Event, Journal, JournalOrder, Problem};
use crate::json::Value;

#[cfg(doc)]
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

/// The records appended to a journal after those a projection was made from, in the order they
/// were read: a journal of them alone, judged by its rules once the projection takes them in.
#[derive(Clone, Debug, Default)]
pub struct Appended {
    journal: Journal,
}

impl Appended {
    /// Whether the batch contains neither records nor read errors.
    pub(crate) fn is_empty(&self) -> bool {
        self.journal.raw().is_empty() && self.journal.invalid.is_empty()
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
            self.journal.report(Problem {
                event: None,
                origin: origin.to_owned(),
                problem: "segment ends in a line not yet whole".to_owned(),
            });
        }
        self.journal.add_segment(data, replica, origin);
    }

    /// Adds the events of `data`, an attempt file found in `replica`'s journal at `origin`, as a
    /// journal reads one ([`Journal::add_attempt`]).
    pub fn add_attempt(&mut self, data: &[u8], replica: &str, origin: &str) {
        self.journal.add_attempt(data, replica, origin);
    }

    /// Adds a record built in memory, with the order its file gives it (a journal's records
    /// sequenced, an attempt file's not).
    pub fn add(&mut self, record: Record) {
        let order = if record.sequenced {
            JournalOrder::Sequenced
        } else {
            JournalOrder::Attempt
        };
        self.journal
            .add(record.value, &record.replica, &record.origin, order);
    }

    /// Whether nothing added was found unreadable so far: a line that is not JSON, an attempt line
    /// that is no valid event, a record with no readable id. A projection declines a batch with
    /// one, as it does one whose events fail as the journal settles them.
    pub fn is_readable(&self) -> bool {
        self.journal.invalid.is_empty()
    }

    /// Every record with a readable id, in the order added.
    pub(crate) fn records(&self) -> &[Event] {
        self.journal.raw()
    }

    /// The journal of them, settled.
    pub(crate) fn settled(self) -> Journal {
        let mut journal = self.journal;
        journal.settle();
        journal
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
    records.into_iter().collect::<Appended>().journal
}
