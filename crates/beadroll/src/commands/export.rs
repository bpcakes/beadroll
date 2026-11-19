//! `bead export`: the tracker as Beads JSONL, br 0.5.7-compatible, to standard output or a file.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt as _;

use beadroll_core::json::{self, Format, Object, Value};
use beadroll_core::text::printed_text;
use beadroll_store::{FileMode, normalize_path, view, write_atomic};

use super::{Context, Failure, Outcome, need_tracker};

/// One compact JSON line per issue; written to `output` when given (its directory's modes, as
/// any file there), else to standard output as it is.
pub(crate) fn export(ctx: &Context<'_>, output: Option<&OsStr>) -> Result<Outcome, Failure> {
    let st = view(ctx.ws)?;
    // before any tracker there is nothing to export: never an empty file over the one given
    need_tracker(&st)?;
    let mut text = String::new();
    for record in st.hydrated()?.export_records() {
        let line = json::write(&Value::from(record), Format::COMPACT)?;
        text.push_str(&line);
        text.push('\n');
    }
    let Some(output) = output else {
        return Ok(Outcome::raw(text).answered_from(st));
    };
    // `-o ""` names no file: refused, never standard output instead
    if output.is_empty() {
        return Err(Failure::usage(
            "-o needs a file: leave it out to write to standard output",
            "",
        ));
    }
    // Normalise the output path before writing: `dump.jsonl/` names `dump.jsonl`.
    write_atomic(
        &normalize_path(output),
        text.as_bytes(),
        FileMode::Checkout,
        &ctx.ws.settings.failpoints,
    )?;
    let count = st.issues().count();
    let shown = printed_text(output.as_bytes());
    let mut result = Object::new();
    result.insert("exported".into(), Value::from(count as i64));
    result.insert("path".into(), Value::from(shown.as_str()));
    Ok(Outcome::new(result, vec![format!("Exported {count} issues to {shown}")]).answered_from(st))
}
