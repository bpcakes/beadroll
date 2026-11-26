//! Reads segment and attempt lines, reporting unreadable content: into the journal itself, each
//! line read straight into what the journal keeps of a record ([`Journal::add_segment`],
//! [`Journal::add_attempt`], roll-lqjj.53.1); or into values, for a tool that needs a record as
//! it was written ([`read_segment`], [`read_attempt`]). Both read a file by the same rules.

use super::content::Digest;
use super::{Admitted, Journal, Problem};
use crate::event::{Envelope, checked_attempt};
use crate::journal::JournalOrder;
use crate::json::{self, ParseError, Value};
use crate::text::strip_bytes;

/// Reads one journal or attempt line. A line that is not strict JSON (not UTF-8, a lone
/// surrogate, a number beyond a float, nesting beyond [`json::MAX_DEPTH`]) is an error the
/// caller reports.
fn loads_line(line: &[u8]) -> Result<Value, ParseError> {
    json::parse(line).map(|parsed| parsed.value)
}

/// Reads a segment's lines with `read`: the records of each, in order. A last line not yet whole
/// is skipped, since it may still be undergoing append; any other unreadable line is reported.
fn segment_lines<T>(
    data: &[u8],
    origin: &str,
    mut problems: Option<&mut Vec<Problem>>,
    mut read: impl FnMut(&[u8]) -> Result<T, ParseError>,
) -> Vec<T> {
    let lines: Vec<&[u8]> = data.split(|b| *b == b'\n').collect();
    let mut out = Vec::new();
    for (number, line) in lines.iter().enumerate() {
        if strip_bytes(line).is_empty() {
            continue;
        }
        match read(line) {
            Ok(record) => out.push(record),
            Err(_) if number + 1 == lines.len() => break,
            Err(_) => {
                if let Some(problems) = problems.as_deref_mut() {
                    let origin = format!("{origin}:{}", number + 1);
                    problems.push(Problem::new(None, origin, "line is not valid JSON"));
                }
            }
        }
    }
    out
}

/// The original reason is enough for catch-up. Only a diagnostic consumer needs the
/// path, line number and whole-file explanation joined into a journal problem.
pub(crate) struct BadAttempt {
    line: usize,
    reason: String,
}

impl BadAttempt {
    fn at(self, origin: &str) -> Problem {
        Problem::new(
            None,
            format!("{origin}:{}", self.line),
            format!("{}; the whole attempt is ignored", self.reason),
        )
    }
}

/// Reads an entire attempt before returning any of its records.
fn attempt_lines<T>(
    data: &[u8],
    mut read: impl FnMut(&[u8]) -> Result<T, String>,
) -> Result<Vec<T>, BadAttempt> {
    let mut out = Vec::new();
    for (number, line) in data.split(|b| *b == b'\n').enumerate() {
        if strip_bytes(line).is_empty() {
            continue;
        }
        let record = read(line).map_err(|reason| BadAttempt {
            line: number + 1,
            reason,
        })?;
        out.push(record);
    }
    Ok(out)
}

fn invalid_json(_: ParseError) -> String {
    "line is not valid JSON".to_owned()
}

/// Reads a segment into the records as written. Skip an unreadable final line that may still be
/// undergoing append; report any other unreadable line.
pub fn read_segment(data: &[u8], origin: &str, problems: Option<&mut Vec<Problem>>) -> Vec<Value> {
    segment_lines(data, origin, problems, loads_line)
}

/// Reads an attempt as a whole into the records as written: any bad line is reported and
/// invalidates the entire attempt.
pub fn read_attempt(data: &[u8], origin: &str, problems: Option<&mut Vec<Problem>>) -> Vec<Value> {
    match attempt_lines(data, |line| {
        checked_attempt(loads_line(line).map_err(invalid_json)?)
    }) {
        Ok(records) => records,
        Err(problem) => {
            if let Some(problems) = problems {
                problems.push(problem.at(origin));
            }
            Vec::new()
        }
    }
}

/// A line read into what the journal keeps: what was read of it, and its content's digest.
type Line = (Envelope, Digest);

impl Journal {
    /// Reads `line` into what this journal keeps of it.
    fn read_line(&self, line: &[u8]) -> Result<Line, ParseError> {
        crate::work::tally(crate::work::Work::JournalLine, 1);
        let envelope = json::deserialize(line)?;
        Ok((envelope, self.shared.digester.digest(line)))
    }

    /// Adds the records of a segment found in `replica`'s journal at `origin`, each line read
    /// straight into what the journal keeps of it, as [`read_segment`] reads them: a last line not
    /// yet whole is skipped, any other unreadable line reported.
    pub fn add_segment(&mut self, data: &[u8], replica: &str, origin: &str) {
        let mut problems = Vec::new();
        let lines = segment_lines(data, origin, Some(&mut problems), |line| {
            self.read_line(line)
        });
        self.invalid.append(&mut problems);
        for (envelope, digest) in lines {
            self.add_for_replay(envelope, digest, replica, origin, JournalOrder::Sequenced);
        }
    }

    /// Adds a segment tail to a catch-up batch. On failure the caller discards the journal,
    /// so there is no need to read later lines or accumulate diagnostics for full replay.
    pub(crate) fn try_add_segment(&mut self, data: &[u8], replica: &str, origin: &str) -> bool {
        if data.last().is_some_and(|last| *last != b'\n') {
            return false;
        }
        for line in data.split(|b| *b == b'\n') {
            if strip_bytes(line).is_empty() {
                continue;
            }
            let Ok((envelope, digest)) = self.read_line(line) else {
                return false;
            };
            let Ok(record) = Admitted::for_batch(envelope, JournalOrder::Sequenced) else {
                return false;
            };
            self.add_read(record, digest, replica, origin);
        }
        true
    }

    /// Adds the events of an attempt file found in `replica`'s journal at `origin`, each line
    /// read straight into what the journal keeps of it, as [`read_attempt`] reads them: all of
    /// them, or none, since any bad line is reported and invalidates the entire attempt.
    pub fn add_attempt(&mut self, data: &[u8], replica: &str, origin: &str) {
        if let Err(problem) = self.try_add_attempt(data, replica, origin) {
            self.invalid.push(problem.at(origin));
        }
    }

    pub(crate) fn try_add_attempt(
        &mut self,
        data: &[u8],
        replica: &str,
        origin: &str,
    ) -> Result<(), BadAttempt> {
        let records = attempt_lines(data, |line| {
            let (envelope, digest) = self.read_line(line).map_err(invalid_json)?;
            Ok((
                Admitted::for_batch(envelope, JournalOrder::Attempt)?,
                digest,
            ))
        })?;
        for (record, digest) in records {
            self.add_read(record, digest, replica, origin);
        }
        Ok(())
    }

    /// Admits a value to a catch-up batch by the same rule as a line from its file.
    pub(crate) fn try_add_record(
        &mut self,
        value: Value,
        replica: &str,
        origin: &str,
        order: JournalOrder,
    ) -> bool {
        let Some(digest) = self.shared.digester.readable_digest(&value) else {
            value.discard();
            return false;
        };
        let envelope = Envelope::from_value(value);
        let Ok(record) = Admitted::for_batch(envelope, order) else {
            return false;
        };
        self.add_read(record, digest, replica, origin);
        true
    }
}
