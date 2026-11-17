//! `bead recap`'s arguments as the command line decides them: the layout it was asked for, whether
//! what it shows is marked seen, and now, which only the command line reads.

use super::parse::Namespace;
use crate::cli::OutputMode;
use beadroll_store::Clock;

use crate::commands::{Failure, Layout, RecapArgs};

/// `recap`'s arguments. What a person reads on a terminal is marked seen once it is written; what
/// a pipe, a file or `--json` takes in (an agent's run, a `| head`) only when `--advance` asks.
pub(super) fn args(
    ns: &Namespace,
    output: OutputMode,
    clock: &Clock,
) -> Result<RecapArgs, Failure> {
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
    let terminal = output == OutputMode::Human && rustix::termios::isatty(std::io::stdout());
    Ok(RecapArgs {
        all: ns.flag("all"),
        advance: ns.flag("advance") || (terminal && !ns.flag("no_advance")),
        layout,
        now: now_millis(clock),
    })
}

/// Milliseconds since the epoch on the real clock ([`Clock::real_ns`]); before it, none.
fn now_millis(clock: &Clock) -> u64 {
    u64::try_from(clock.real_ns() / 1_000_000).unwrap_or(0)
}
