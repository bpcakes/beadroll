//! `bead export`: the tracker as Beads JSONL, br 0.5.7-compatible, to standard output or a file.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt as _;

use beadroll_core::code::Code;
use beadroll_core::json::{self, Format, Object, Value};
use beadroll_core::text::printed_text;
use beadroll_store::{pathlib, view, write_atomic};

use super::{Context, Failure, Outcome};

/// One compact JSON line per issue; written to `output` when given (its directory's modes, as
/// any file there), else to standard output as it is.
pub(crate) fn export(ctx: &Context<'_>, output: Option<&OsStr>) -> Result<Outcome, Failure> {
    let st = view(ctx.ws)?;
    let mut text = String::new();
    for record in st.export_records() {
        let line = json::write(&Value::from(record), Format::COMPACT)
            .map_err(|e| Failure::new(Code::Validation, e.to_string()))?;
        text.push_str(&line);
        text.push('\n');
    }
    let Some(output) = output.filter(|o| !o.is_empty()) else {
        return Ok(Outcome::raw(text));
    };
    // written where Python's `Path(args.output)` points: `dump.jsonl/` is `dump.jsonl`
    write_atomic(
        &pathlib(output),
        text.as_bytes(),
        false,
        &ctx.ws.settings.failpoints,
    )?;
    let count = st.issues().count();
    let shown = printed_text(output.as_bytes());
    let mut result = Object::new();
    result.insert("exported".into(), Value::from(count as i64));
    result.insert("path".into(), Value::from(shown.as_str()));
    Ok(Outcome::new(
        result,
        vec![format!("Exported {count} issues to {shown}")],
    ))
}
