//! A failure or a warning on standard error: JSON with `--json`, else a painted line.

use beadroll_core::code::Code;
use beadroll_core::json::{self, Format, Object, Value};
use beadroll_core::text::json_ascii_bytes;

use super::OutputMode;
use super::output::{Stream, emit};
use super::paint::{self, Color};
use crate::commands::{Failure, Line, Tone};

/// Reports a post-commit count warning separately, preserving every command's stdout shape.
pub(super) fn warn(failure: &Failure, output: OutputMode, color_mode: Color) {
    let text = if output == OutputMode::Json {
        let raw = failure.raw.as_deref().unwrap_or(failure.message.as_bytes());
        format!(
            "{{\"warning\": {{\"code\": {}, \"message\": {}}}}}",
            json_ascii_bytes(failure.code.as_str().as_bytes()),
            json_ascii_bytes(raw)
        )
    } else {
        let line = Line::toned(Tone::Warning, "Warning:")
            .with(Tone::Plain, " ")
            .with(Tone::Plain, failure.printed());
        paint::render_all(&[line], color_mode)
    };
    // A warning cannot turn an already committed command into a failure, even if stderr is gone.
    let _ = emit(Some(&text), Stream::Stderr);
}

/// Reports `failure` on standard error: `{"error": {"code", "message"}}` with `--json`, else
/// `Error: message`, painted as `color_mode` says. The status is the command's (2 for `USAGE`, else
/// 1), whatever stderr does.
pub(super) fn report(failure: &Failure, output: OutputMode, color_mode: Color) -> u8 {
    let text = match (output == OutputMode::Json, &failure.raw) {
        // Bytes that are not UTF-8 become U+FFFD.
        (true, Some(raw)) => format!(
            "{{\"error\": {{\"code\": {}, \"message\": {}}}}}",
            json_ascii_bytes(failure.code.as_str().as_bytes()),
            json_ascii_bytes(raw)
        ),
        (true, None) => {
            let mut error = Object::new();
            error.insert("code".into(), Value::from(failure.code.as_str()));
            error.insert("message".into(), Value::from(failure.message.as_str()));
            let mut body = Object::new();
            body.insert("error".into(), Value::from(error));
            json::write(&Value::from(body), Format::ASCII).unwrap_or_default()
        }
        (false, _) => {
            let mut lines = vec![
                Line::toned(Tone::Error, "Error:")
                    .with(Tone::Plain, " ")
                    .with(Tone::Plain, failure.printed()),
            ];
            if let Some(after) = &failure.after {
                lines.push(Line::new());
                lines.extend(
                    after
                        .split('\n')
                        .map(|line| match line.strip_prefix("Usage:") {
                            Some(usage) => {
                                Line::toned(Tone::Heading, "Usage:").with(Tone::Plain, usage)
                            }
                            None => Line::from(line),
                        }),
                );
            }
            paint::render_all(&lines, color_mode)
        }
    };
    let _ = emit(Some(&text), Stream::Stderr);
    if failure.code == Code::Usage { 2 } else { 1 }
}
