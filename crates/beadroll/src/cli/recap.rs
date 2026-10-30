//! `bead recap`'s arguments as the command line decides them: the layout it was asked for, whether
//! what it shows is marked seen, and now, which only the command line reads.

use super::parse::Namespace;
use crate::cli::OutputMode;
use crate::commands::{Failure, Layout, RecapArgs};

/// `recap`'s arguments. What a person reads on a terminal is marked seen once it is written; what
/// a pipe, a file or `--json` takes in (an agent's run, a `| head`) only when `--advance` asks.
pub(super) fn args(ns: &Namespace, as_json: OutputMode) -> Result<RecapArgs, Failure> {
    let as_json = matches!(as_json, OutputMode::Json);
    let layout = match ns.text("format") {
        None => Layout::Events,
        Some(name) => (name.to_str().and_then(Layout::named)).ok_or_else(|| {
            Failure::usage(
                format!(
                    "invalid value '{}' for '--format <FORMAT>' [possible values: events, \
                     issues, full]",
                    name.to_string_lossy()
                ),
                "",
            )
        })?,
    };
    let terminal = !as_json && rustix::termios::isatty(std::io::stdout());
    Ok(RecapArgs {
        all: ns.flag("all"),
        advance: ns.flag("advance") || (terminal && !ns.flag("no_advance")),
        layout,
        now: now_millis(),
    })
}

/// Milliseconds since the epoch on the real clock.
fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}
