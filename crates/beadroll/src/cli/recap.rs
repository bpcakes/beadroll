//! Recap acknowledges only the answer that reached stdout, after the final flush.

use std::sync::Arc;

use beadroll_core::json::{Format, Value};
use beadroll_store::Settings;

use super::output::{Unwritten, emit_delivered};
use super::{Namespace, paint, write};
use crate::commands::{self, Failure};

pub(super) fn run(
    settings: Arc<Settings>,
    ns: &Namespace,
    as_json: bool,
    color: bool,
) -> Result<(), Failure> {
    let (outcome, cursor) = commands::recap(settings, ns.flag("all"), !ns.flag("no_advance"))?;
    let text = if as_json {
        write(&outcome.result.unwrap_or(Value::Null), Format::SPACED)?
    } else {
        paint::render_all(&outcome.lines.unwrap_or_default(), color)
    };
    match emit_delivered(&text) {
        Ok(()) => cursor.save().map_err(Failure::from),
        // A closed pipe keeps bead's quiet exit, but it must not consume unseen activity.
        Err(Unwritten::Gone) => Ok(()),
        Err(Unwritten::Failed(error)) => Err(beadroll_git::IoError::bare(&error).into()),
    }
}
