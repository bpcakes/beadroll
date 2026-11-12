//! A sync round's progress and outcome, and the recovery allowed between rounds.

use beadroll_core::json::Object;
use beadroll_git::PushOutcome;
use beadroll_store::{Workspace, load_pending, segment_files};

use super::{SyncReport, nothing_published, refused};
use crate::PublishMode;
use crate::error::Error;
use crate::link::{Link, Merged};
use crate::tx::{drop_pending, locked};

/// A merge can bring events in before publication fails. Keep that progress on every outcome
/// so a later round's successful report includes what came in before a rebuild.
pub(super) struct Round {
    pub pulled: i64,
    pub outcome: Result<Outcome, Error>,
}

pub(super) enum Outcome {
    Published(Published),
    Contended,
}

/// Only the successful round contributes its outgoing count and maintenance result.
pub(super) struct Published {
    pub pushed: i64,
    pub repacked: bool,
    pub counts: Object,
    pub merged: Box<Merged>,
}

/// Independent of the path Link keeps for the report: recovery spends this allowance once.
pub(super) enum Rebuild {
    Available,
    Used,
}

pub(super) enum AfterError {
    Rebuild,
    ConsiderAdoption,
    Stop,
}

impl Rebuild {
    pub(super) fn after_error(&mut self, attempt: u32, error: &Error) -> AfterError {
        if matches!(error, Error::SyncRepoDamaged(_)) && matches!(self, Self::Available) {
            *self = Self::Used;
            AfterError::Rebuild
        } else if attempt == 1 {
            AfterError::ConsiderAdoption
        } else {
            AfterError::Stop
        }
    }
}

/// Fetch before locking, and fetch again under the lock when pending attempts need the current
/// head. All merge, fence, retirement, maintenance and publication counting stays under that
/// same lock.
pub(super) fn sync_round(ws: &Workspace, link: &mut Link<'_>, report: SyncReport) -> Round {
    let mut pulled = 0;
    let mut discarded = None;
    let outcome = (|| {
        let mut tip = link.fetch()?;
        locked(ws, || {
            if !load_pending(ws)?.is_empty() {
                // Also sweeps pending files that are not attempts; settling our own attempts
                // needs the current head, not a snapshot.
                tip = link.fetch()?;
            }
            let merged = link.merge_from(tip.as_deref(), report)?;
            pulled = merged.pulled;
            let publication = (|| {
                if tip.is_none() && segment_files(ws)?.is_empty() {
                    return Err(nothing_published(ws, &link.endpoint()));
                }
                let pushed = link.unpublished_after(&merged)?;
                let fence = if load_pending(ws)?.is_empty() {
                    PublishMode::IfChanged
                } else {
                    PublishMode::Fence
                };
                match link.publish(tip.as_deref(), "sync", &[], fence)? {
                    PushOutcome::Ok => {
                        // Absent at a head we just advanced: they can never land.
                        drop_pending(ws, &load_pending(ws)?)?;
                        let repacked = link.maintain()?;
                        // Later offline writes must not be counted as part of this publication.
                        let counts = link.published_counts()?;
                        Ok(Some((pushed, repacked, counts)))
                    }
                    PushOutcome::Refused => Err(refused(link)),
                    PushOutcome::Uncertain => Err(Error::RemoteUnreachable(format!(
                        "the push did not complete; nothing was lost, run sync again once the cause is \
                     fixed: {}",
                        link.last_error
                    ))),
                    PushOutcome::Rejected => Ok(None),
                }
            })();
            match publication {
                Ok(Some((pushed, repacked, counts))) => Ok(Outcome::Published(Published {
                    pushed,
                    repacked,
                    counts,
                    merged: Box::new(merged),
                })),
                other => {
                    // A failed or contended round also frees its view after unlocking.
                    discarded = Some(merged);
                    other.map(|_| Outcome::Contended)
                }
            }
        })
    })();
    Round { pulled, outcome }
}
