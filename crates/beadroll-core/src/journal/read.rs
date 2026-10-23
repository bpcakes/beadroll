//! Reads segment and attempt lines, reporting unreadable content: into the journal itself, each
//! line read straight into what the journal keeps of a record ([`Journal::add_segment`],
//! [`Journal::add_attempt`], roll-lqjj.53.1); or into values, for a tool that needs a record as
//! it was written ([`read_segment`], [`read_attempt`]). Both read a file by the same rules.

use crate::event::{Envelope, event_problem, payload_problem};
use crate::json::{self, ParseError, Value};
use crate::text::strip_bytes;

use super::content::Digest;
use super::{Journal, Problem};

/// Reads one journal or attempt line. A line that is not strict JSON (not UTF-8, a lone
/// surrogate, a number beyond a float, nesting beyond [`json::MAX_DEPTH`]) is an error the
/// caller reports.
pub fn loads_line(line: &[u8]) -> Result<Value, ParseError> {
    json::parse(line).map(|parsed| parsed.value)
}

/// What an attempt line holds, judged whole: a line that is not strict JSON, not an object, or
/// not a valid event is the problem with it.
type Judged<T> = Result<T, String>;

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

/// Reads an attempt's lines with `read`, which judges each whole: the events of all of them, or
/// none, since any bad line is reported and invalidates the entire attempt.
fn attempt_lines<T>(
    data: &[u8],
    origin: &str,
    mut problems: Option<&mut Vec<Problem>>,
    mut read: impl FnMut(&[u8]) -> Judged<T>,
) -> Vec<T> {
    let mut out = Vec::new();
    for (number, line) in data.split(|b| *b == b'\n').enumerate() {
        if strip_bytes(line).is_empty() {
            continue;
        }
        match read(line) {
            Ok(record) => out.push(record),
            Err(problem) => {
                if let Some(problems) = problems.as_deref_mut() {
                    let origin = format!("{origin}:{}", number + 1);
                    problems.push(Problem::new(
                        None,
                        origin,
                        format!("{problem}; the whole attempt is ignored"),
                    ));
                }
                return Vec::new();
            }
        }
    }
    out
}

/// An attempt line's event, judged whole: `read` reads the line, and `problem` checks it by
/// reference so a reader that keeps the written value need not copy it for validation.
fn judged<T>(
    line: &[u8],
    read: impl FnOnce(&[u8]) -> Result<T, ParseError>,
    problem: impl FnOnce(&T) -> Option<String>,
) -> Judged<T> {
    let Ok(record) = read(line) else {
        return Err("line is not valid JSON".to_owned());
    };
    match problem(&record) {
        None => Ok(record),
        Some(problem) => Err(problem),
    }
}

/// Reads a segment into the records as written. Skip an unreadable final line that may still be
/// undergoing append; report any other unreadable line.
pub fn read_segment(data: &[u8], origin: &str, problems: Option<&mut Vec<Problem>>) -> Vec<Value> {
    segment_lines(data, origin, problems, loads_line)
}

/// Reads an attempt as a whole into the records as written: any bad line is reported and
/// invalidates the entire attempt.
pub fn read_attempt(data: &[u8], origin: &str, problems: Option<&mut Vec<Problem>>) -> Vec<Value> {
    attempt_lines(data, origin, problems, |line| {
        judged(line, loads_line, |value| {
            if !matches!(value, Value::Object(_)) {
                Some("attempt line is not an event".to_owned())
            } else {
                event_problem(value).or_else(|| payload_problem(value))
            }
        })
    })
}

/// A line read into what the journal keeps: what was read of it, and its content's digest.
type Line = (Envelope, Digest);

impl Journal {
    /// Reads `line` into what this journal keeps of it.
    fn read_line(&self, line: &[u8]) -> Result<Line, ParseError> {
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
            self.add_read(envelope, digest, replica, origin, true);
        }
    }

    /// Adds the events of an attempt file found in `replica`'s journal at `origin`, each line
    /// read straight into what the journal keeps of it, as [`read_attempt`] reads them: all of
    /// them, or none, since any bad line is reported and invalidates the entire attempt.
    pub fn add_attempt(&mut self, data: &[u8], replica: &str, origin: &str) {
        let mut problems = Vec::new();
        let lines = attempt_lines(data, origin, Some(&mut problems), |line| {
            judged(
                line,
                |line| self.read_line(line),
                |(envelope, _)| {
                    if !envelope.object {
                        Some("attempt line is not an event".to_owned())
                    } else {
                        envelope.problem().or_else(|| envelope.payload_problem())
                    }
                },
            )
        });
        self.invalid.append(&mut problems);
        for (envelope, digest) in lines {
            self.add_read(envelope, digest, replica, origin, false);
        }
    }
}
