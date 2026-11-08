//! Ownership changes are conditional updates against the current remote head. Write a receipt and
//! pending attempt first; they count only after remote acceptance. Drop definitely refused
//! pushes. Retain unknown outcomes until a later fetch confirms them or a fence on that head
//! prevents them from landing. Receipts recognise accepted retries by the same actor without
//! applying twice. Backoff never holds the store lock.

use std::ffi::OsStr;

use beadroll_core::json::Value;
use beadroll_git::PushOutcome;
use beadroll_store::IdentityMode;
use beadroll_store::PublicationState;
use beadroll_store::{Actor, Endpoint, Workspace, load_pending, replica_id};

use crate::NetworkMode;
use crate::PublishMode;
use crate::error::Error;
use crate::link::Link;
use crate::receipts::{Request, RetireAfterSuccess, settle};
use crate::synchronise::{ROUNDS, conflict_detail, pause, refused};
use crate::tx::{Spent, Tx, drop_pending, locked, writing};

mod attempt;
use attempt::{AttemptOutcome, PendingAttempt, Recovery};

/// How an ownership change was made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Coordinated with the remote, which accepted it.
    Online,
    /// With `--offline`: provisional, not exclusive until claimed online.
    Offline,
    /// On a tracker with no remote.
    Local,
}

impl Mode {
    /// The mode name: online, offline or local.
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Online => "online",
            Mode::Offline => "offline",
            Mode::Local => "local",
        }
    }
}

/// The result of settling this machine's pending attempts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Settled {
    /// None is left: each was confirmed, or fenced off and dropped.
    Clear,
    /// The head moved before the fence landed: fetch again.
    Moved,
    /// The fence's push did not complete: they stay unsettled.
    Unresolved,
}

/// Settles unknown local attempts after merging tip has confirmed accepted ones. Fence remaining
/// attempts, which were absent at tip, with a commit on that head before dropping them, so an
/// in-flight push cannot land later. Requires the store lock.
pub fn settle_pending(
    ws: &Workspace,
    link: &mut Link<'_>,
    tip: Option<&str>,
) -> Result<Settled, Error> {
    let pending = load_pending(ws)?;
    if pending.is_empty() {
        return Ok(Settled::Clear);
    }
    match link.publish(tip, "fence", &[], PublishMode::Fence)? {
        PushOutcome::Ok => {
            drop_pending(ws, &pending)?;
            Ok(Settled::Clear)
        }
        // the attempts stay pending: the fence that would settle them did not land
        PushOutcome::Refused => Err(refused(link)),
        PushOutcome::Rejected => Ok(Settled::Moved),
        PushOutcome::Uncertain => Ok(Settled::Unresolved),
    }
}

/// Builds and conditionally publishes an ownership change against the current remote head.
/// Offline mode or no remote makes it a local write. A matching accepted receipt signature
/// returns the previous result without executing again.
///
/// Failpoints mark the start of the transaction on the merge's projection
/// (`ownership.before-transaction`), the persisted receipt and attempt (`ownership.before-publish`),
/// impending push (`claim.before-push`), acceptance (`ownership.after-publish-ok`), local
/// confirmation (`ownership.after-confirm`) and backoff. An error after confirmation leaves the
/// pending twin in place.
pub fn coordinated<E: From<Error>>(
    ws: &Workspace,
    actor: &Actor,
    network_mode: NetworkMode,
    remote: Option<&OsStr>,
    signature: &str,
    mut build: impl FnMut(&mut Tx<'_>, Mode) -> Result<Mode, E>,
) -> Result<Mode, E> {
    let remote = match connection(ws, network_mode, remote).map_err(E::from)? {
        Connection::Remote(remote) => remote,
        Connection::Local(mode) => return writing(ws, actor, |tx| build(tx, mode)),
    };
    let mut link = Link::new(ws, &remote).map_err(E::from)?;
    let replica = replica_id(&ws.settings, IdentityMode::Create)
        .map_err(|e| E::from(e.into()))?
        .unwrap_or_default();
    // a push of ours that got no answer, with what it answered: it may still have landed
    let mut recovery = Recovery::Ready;
    // this actor's settled receipts: dropped only once this command has gone through
    let mut stale = RetireAfterSuccess::default();
    // Preparing consumes each round's transaction. Its view and any unwritten events are let go
    // of after the lock, as `writing` lets go of its own, including on a failed build.
    let mut spent = Spent::default();
    let mut pending_write = None;
    for attempt in 1..=ROUNDS {
        let mut unused_merge = None;
        let round = locked(ws, || -> Result<Option<Mode>, Round<E>> {
            // under the lock: ownership is decided against the head as it is now
            let tip = link.fetch()?;
            let merged = link.merge_from(tip.as_deref(), crate::synchronise::SyncReport::Skip)?;
            let before = (|| -> Result<BeforeBuild, Round<E>> {
                if let Some(answer) = recovery.confirmed(ws)? {
                    // it had landed: the work is done, not to be redone
                    stale.retire()?;
                    // The merge confirmed this request without going through confirm_ownership.
                    ws.record_commit();
                    published(ws, &mut link, &remote)?;
                    return Ok(BeforeBuild::Done(Some(answer)));
                }
                let settled = settle(ws, &replica, actor, signature)?;
                stale = settled.retire;
                if settled.request == Request::Accepted {
                    // an earlier run of this command was accepted
                    ws.record_commit();
                    published(ws, &mut link, &remote)?;
                    return Ok(BeforeBuild::Done(Some(Mode::Online)));
                }
                match settle_pending(ws, &mut link, tip.as_deref())? {
                    Settled::Unresolved => return Err(unresolved(&link).into()),
                    Settled::Moved => return Ok(BeforeBuild::Done(None)),
                    Settled::Clear => {}
                }
                Ok(BeforeBuild::Build)
            })();
            match before {
                Ok(BeforeBuild::Build) => {}
                Ok(BeforeBuild::Done(answer)) => {
                    unused_merge = Some(merged);
                    return Ok(answer);
                }
                Err(error) => {
                    unused_merge = Some(merged);
                    return Err(error);
                }
            }
            // whatever this command had in flight is settled one way or the other
            recovery = Recovery::Ready;
            // nothing between the merge and here writes the journal, so the projection the merge
            // left is the store's (roll-lqjj.29); `Tx::on` reads the store again if something did
            // (roll-lqjj.37)
            ws.settings.failpoints.hit("ownership.before-transaction");
            let mut tx = Tx::on(ws, actor, merged.state)?;
            let built = build(&mut tx, Mode::Online).map_err(Round::Build);
            let (prepared, retained) = tx.prepare();
            spent = retained;
            let prepared = pending_write.insert(prepared);
            let answer = built?;
            prepared.append(ws)?;
            if prepared.ownership.is_empty() {
                stale.retire()?; // nothing to settle: this command went through
                return Ok(Some(answer));
            }
            let pending = PendingAttempt::prepare(
                ws,
                std::mem::take(&mut prepared.ownership),
                signature,
                answer,
            )?;
            let outcome = pending.publish(ws, &mut link, tip.as_deref())?;
            match outcome {
                AttemptOutcome::Confirmed(answer) => {
                    // this command went through: the actor has moved on from those
                    stale.retire()?;
                    published(ws, &mut link, &remote)?;
                    Ok(Some(answer))
                }
                AttemptOutcome::Refused => Err(refused(&link).into()),
                // someone else got there first; decide again against the new head
                AttemptOutcome::Rejected => Ok(None),
                AttemptOutcome::Uncertain(pending) => {
                    recovery = Recovery::Awaiting(pending);
                    Ok(None)
                }
            }
        });
        drop(std::mem::take(&mut spent));
        drop(pending_write.take());
        match round {
            Ok(Some(answer)) => return Ok(answer),
            Ok(None) => {}
            Err(Round::Sync(Error::SyncRepoDamaged(_))) if link.rebuilt.is_none() => {
                // once: set the sync repository aside and decide again
                locked(ws, || link.rebuild()).map_err(E::from)?;
                continue;
            }
            Err(Round::Sync(error)) => return Err(error.into()),
            Err(Round::Build(error)) => return Err(error),
        }
        pause(attempt, &ws.settings.failpoints);
    }
    Err(contended(&link).into())
}

/// Why a round of [`coordinated`] stopped: the protocol's own failure, which one rebuild of a
/// damaged sync repository may cure, or the command's.
enum Round<E> {
    Sync(Error),
    Build(E),
}

fn contended(link: &Link<'_>) -> Error {
    Error::SyncConflict(format!(
        "the remote journal kept moving; try again{}",
        conflict_detail(link)
    ))
}

fn unresolved(link: &Link<'_>) -> Error {
    Error::Unresolved(format!(
        "an ownership attempt is unsettled and the push that would settle it did \
         not complete ({}); it counts for nothing until `bead sync` can settle it",
        link.last_error
    ))
}

impl<E> From<Error> for Round<E> {
    fn from(error: Error) -> Round<E> {
        Round::Sync(error)
    }
}

/// The remote has all of this store: the counts it holds, and nothing left unpublished.
fn published(ws: &Workspace, link: &mut Link<'_>, remote: &Endpoint) -> Result<(), Error> {
    let counts = link.published_counts()?;
    ws.note_sync(remote, &[("published", Value::from(counts))]);
    ws.set_wrote(PublicationState::Published);
    Ok(())
}

/// Settlement either allows a fresh transaction or finishes this round (retrying on `None`).
enum BeforeBuild {
    Build,
    Done(Option<Mode>),
}

/// Whether this request reaches a coordination remote or writes provisionally on this machine.
enum Connection {
    Remote(Endpoint),
    Local(Mode),
}

fn connection(
    ws: &Workspace,
    network_mode: NetworkMode,
    remote: Option<&OsStr>,
) -> Result<Connection, Error> {
    let remote = ws.endpoint(remote)?.filter(|remote| !remote.url.is_empty());
    Ok(match remote {
        Some(remote) if network_mode == NetworkMode::Online => Connection::Remote(remote),
        Some(_) => Connection::Local(Mode::Offline),
        None => Connection::Local(Mode::Local),
    })
}
