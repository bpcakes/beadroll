//! Reads segment and attempt lines into events, reporting unreadable content.

use crate::event::{event_problem, payload_problem};
use crate::json::{self, ParseError, Value};
use crate::text::strip_bytes;

use super::Problem;

/// Reads one journal or attempt line. A line that is not strict JSON (not UTF-8, a lone
/// surrogate, a number beyond a float, nesting beyond [`json::MAX_DEPTH`]) is an error the
/// caller reports.
pub fn loads_line(line: &[u8]) -> Result<Value, ParseError> {
    json::parse(line).map(|parsed| parsed.value)
}

/// Reads a segment. Skip an unreadable final line that may still be undergoing append; report any
/// other unreadable line.
pub fn read_segment(
    data: &[u8],
    origin: &str,
    mut problems: Option<&mut Vec<Problem>>,
) -> Vec<Value> {
    let lines: Vec<&[u8]> = data.split(|b| *b == b'\n').collect();
    let mut events = Vec::new();
    for (number, line) in lines.iter().enumerate() {
        if strip_bytes(line).is_empty() {
            continue;
        }
        match loads_line(line) {
            Ok(event) => events.push(event),
            Err(_) if number + 1 == lines.len() => break,
            Err(_) => {
                if let Some(problems) = problems.as_deref_mut() {
                    let origin = format!("{origin}:{}", number + 1);
                    problems.push(Problem::new(None, origin, "line is not valid JSON"));
                }
            }
        }
    }
    events
}

/// Reads an attempt as a whole: any bad line is reported and invalidates the entire attempt.
pub fn read_attempt(
    data: &[u8],
    origin: &str,
    mut problems: Option<&mut Vec<Problem>>,
) -> Vec<Value> {
    let mut events = Vec::new();
    for (number, line) in data.split(|b| *b == b'\n').enumerate() {
        if strip_bytes(line).is_empty() {
            continue;
        }
        let problem = match loads_line(line) {
            Err(_) => Some("line is not valid JSON".to_owned()),
            Ok(event) if !matches!(event, Value::Object(_)) => {
                Some("attempt line is not an event".to_owned())
            }
            Ok(event) => match event_problem(&event).or_else(|| payload_problem(&event)) {
                None => {
                    events.push(event);
                    None
                }
                problem => problem,
            },
        };
        if let Some(problem) = problem {
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
    events
}
