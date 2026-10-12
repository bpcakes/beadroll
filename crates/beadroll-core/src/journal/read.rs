//! Reads segment and attempt lines into events, reporting unreadable content.

use crate::event::{event_problem, payload_problem};
use crate::json::{self, ParseError, Value};
use crate::text::strip_bytes;

use super::Problem;

/// Reads one journal or attempt line. Lone surrogates become U+FFFD and are reported because they
/// cannot be printed or exported as text.
pub fn loads_line(
    line: &[u8],
    origin: &str,
    number: usize,
    problems: Option<&mut Vec<Problem>>,
) -> Result<Value, ParseError> {
    let parsed = json::parse(line)?;
    if parsed.lone_surrogates
        && let Some(problems) = problems
    {
        let event = parsed
            .value
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let origin = format!("{origin}:{}", number + 1);
        problems.push(Problem::new(
            event,
            origin,
            "text is not valid UTF-8; it reads as U+FFFD",
        ));
    }
    Ok(parsed.value)
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
        match loads_line(line, origin, number, problems.as_deref_mut()) {
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
        let problem = match loads_line(line, origin, number, problems.as_deref_mut()) {
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
