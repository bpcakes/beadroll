//! `bead sync`: exchange the journal with the other machines through the coordination ref.

use std::ffi::OsString;

use beadroll_core::code::Code;
use beadroll_core::text::str_of;
use beadroll_sync::synchronise;

use super::{Context, Failure, Outcome};

/// Synchronises with the coordination remote (`--remote`, or the one this tracker uses).
pub(crate) fn sync(ctx: &Context<'_>, remote: Option<&OsString>) -> Result<Outcome, Failure> {
    let remote = ctx.ws.endpoint(remote.map(OsString::as_os_str))?;
    let Some(remote) = remote.filter(|remote| !remote.url.is_empty()) else {
        return Err(Failure::new(
            Code::NoRemote,
            "no coordination remote known for this tracker: pass --remote",
        ));
    };
    let synced = synchronise(ctx.ws, &remote)?;
    let mut lines = vec![format!(
        "Synced with {}: pulled {}, pushed {} event(s)",
        remote.url.to_string_lossy(),
        synced.pulled,
        synced.pushed
    )];
    for renamed in &synced.renamed {
        let part = |key: &str| renamed.get(key).map(str_of).unwrap_or_default();
        lines.push(format!(
            "  renamed {} -> {} (created on machine {})",
            part("from"),
            part("to"),
            part("replica")
        ));
    }
    for path in &synced.forked_journals {
        lines.push(format!(
            "  ! {path} differed from the published copy; ours was kept beside it and the \
             published copy used"
        ));
    }
    if synced.integrity > 0 {
        lines.push(format!(
            "  ! {} integrity problem(s); see `bead info`",
            synced.integrity
        ));
    }
    Ok(Outcome::new(synced.to_value(), lines))
}
