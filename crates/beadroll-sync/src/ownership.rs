//! The ownership protocol (Python's `coordinated`, `settle_commands` and `settle_pending`): a
//! claim, a takeover or a release is decided against the remote's head as it is now and published
//! as one conditional update. The attempt (its ownership events, and any edits made with them) is
//! written as pending files first, with a receipt naming the command, and counts for nothing until
//! the remote has it. A push the remote definitely refused is dropped; one whose outcome is unknown
//! stays pending, and the next round (or the next `sync`) finds it on the remote, which confirms
//! it, or fences it off with a commit on the same head and drops it. A retry of the same command by
//! the same actor after a lost acknowledgement is recognised by its receipt and not applied again.
//! Nothing sleeps while the store lock is held.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use beadroll_core::code::Code;
use beadroll_core::json::Value;
use beadroll_git::PushOutcome;
use beadroll_store::{
    Actor, Fired, Workspace, append_events, attempt_path, glob_ending, json_text, load_pending,
    private_dirs, read, replica_id, write_atomic, write_attempt_file,
};

use crate::error::Error;
use crate::link::Link;
use crate::synchronise::{ROUNDS, conflict_detail, pause, published_counts, refused};
use crate::tx::{Tx, confirm_ownership, drop_pending, locked, remove, writing};

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
    /// As Python names it: `online`, `offline` or `local`.
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Online => "online",
            Mode::Offline => "offline",
            Mode::Local => "local",
        }
    }
}

/// What settling this machine's unsettled attempts came to (Python's `settle_pending`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Settled {
    /// None is left: each was confirmed, or fenced off and dropped.
    Clear,
    /// The head moved before the fence landed: fetch again.
    Moved,
    /// The fence's push did not complete: they stay unsettled.
    Unresolved,
}

/// This actor's earlier receipts (Python's `settle_commands`): whether one whose attempt the remote
/// accepted asked for exactly this command (`signature`), so that the command is already done; and
/// the other settled receipts of this actor, which are stale once this command goes through. A
/// receipt whose attempt is still pending stays; one older than an accepted attempt of this actor
/// is dropped now; one that is not a receipt bead wrote is dropped. Lock held.
pub fn settle_commands(
    ws: &Workspace,
    replica: &str,
    actor: &Actor,
    signature: &str,
) -> Result<(bool, Vec<PathBuf>), Error> {
    let mut settled = Vec::new();
    for receipt in glob_ending(&ws.dir.join("pending"), ".command") {
        let recorded = read(&receipt).ok().and_then(|data| json_text(&data).ok());
        let Some(Value::Object(recorded)) = recorded else {
            // unreadable, or not a receipt bead wrote: it can answer nothing
            remove(&receipt)?;
            continue;
        };
        // an actor whose text Python holds as lone surrogates wrote no receipt
        if !actor.writable || recorded.get("actor") != Some(&actor.value) {
            continue;
        }
        if receipt.with_extension("json").exists() {
            continue; // still pending: nothing to conclude yet
        }
        settled.push(receipt);
    }
    let claims = ws.events_dir.join(replica).join("claims");
    let accepted: Vec<&PathBuf> = settled
        .iter()
        .filter(|receipt| claims.join(attempt_name(receipt)).exists())
        .collect();
    let latest = accepted.last().map(|receipt| receipt.file_name());
    let (mut done, mut stale) = (false, Vec::new());
    // named by attempt id (UUIDv7), so in time order
    for receipt in &settled {
        if latest.is_some_and(|latest| receipt.file_name() < latest) {
            // a later command of this actor was accepted: it has moved on
            remove(receipt)?;
        } else if accepted.contains(&receipt) && read(receipt)? == signature.as_bytes() {
            done = true;
            remove(receipt)?;
        } else {
            stale.push(receipt.clone());
        }
    }
    Ok((done, stale))
}

/// The attempt file a receipt answers for: its name with `.json`.
fn attempt_name(receipt: &Path) -> PathBuf {
    PathBuf::from(
        receipt
            .with_extension("json")
            .file_name()
            .unwrap_or_default(),
    )
}

/// Settles this machine's ownership attempts whose outcome is unknown, after a merge against
/// `tip` has confirmed any the remote holds (Python's `settle_pending`): whatever is still pending
/// was absent at `tip`, so a fence commit on that head makes sure an attempt still in flight can no
/// longer land, and only then are they dropped. Lock held.
pub fn settle_pending(
    ws: &Workspace,
    link: &mut Link<'_>,
    tip: Option<&str>,
) -> Result<Settled, Error> {
    let pending = load_pending(ws)?;
    if pending.is_empty() {
        return Ok(Settled::Clear);
    }
    match link.publish(tip, "fence", &[], true)? {
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

/// Removes these receipts: the actor has moved on from them.
pub fn forget(receipts: &[PathBuf]) -> Result<(), Error> {
    receipts.iter().try_for_each(|receipt| remove(receipt))
}

/// Runs an ownership change (`build`, which emits it into a transaction and answers how it was
/// made) against the remote's head as it is now, and publishes it as one conditional update
/// (Python's `coordinated`). `--offline` (`offline`), or a tracker with no remote, makes it a local
/// write instead. `signature` is the command's receipt (Python's `command_signature`): a retry of
/// an accepted command by the same actor is recognised by it and answered without running again.
///
/// Failpoints: `ownership.before-publish` once the receipt and the attempt are written,
/// `claim.before-push` right before the push, `ownership.after-publish-ok` when the remote took
/// it, `ownership.after-confirm` once it counts here (`error(CODE)` fails the command there, its
/// pending twin left in place), and `backoff` between rounds.
pub fn coordinated<E: From<Error>>(
    ws: &Workspace,
    actor: &Actor,
    offline: bool,
    remote: Option<&OsStr>,
    signature: &str,
    mut build: impl FnMut(&mut Tx<'_>, Mode) -> Result<Mode, E>,
) -> Result<Mode, E> {
    let remote = ws.remote(remote).map_err(|e| E::from(e.into()))?;
    let remote = remote.filter(|remote| !remote.is_empty());
    let remote = match remote {
        Some(remote) if !offline => remote,
        known => {
            let mode = if known.is_some() {
                Mode::Offline
            } else {
                Mode::Local
            };
            return writing(ws, actor, |tx| build(tx, mode));
        }
    };
    let mut link = Link::new(ws, &remote).map_err(E::from)?;
    let replica = replica_id(&ws.settings, true)
        .map_err(|e| E::from(e.into()))?
        .unwrap_or_default();
    // a push of ours that got no answer, with what it answered: it may still have landed
    let mut receipt: Option<(Vec<Value>, Mode)> = None;
    // this actor's settled receipts: dropped only once this command has gone through
    let mut stale: Vec<PathBuf> = Vec::new();
    for attempt in 1..=ROUNDS {
        let round = locked(ws, || -> Result<Option<Mode>, Round<E>> {
            // under the lock: ownership is decided against the head as it is now
            let tip = link.fetch()?;
            link.merge(tip.as_deref())?;
            if let Some((events, answer)) = &receipt
                && attempt_path(ws, events).exists()
            {
                // it had landed: the work is done, not to be redone
                drop_pending(ws, std::slice::from_ref(events))?;
                forget(&stale)?;
                published(ws, &remote)?;
                return Ok(Some(*answer));
            }
            let (done, settled) = settle_commands(ws, &replica, actor, signature)?;
            stale = settled;
            if done {
                // an earlier run of this command was accepted
                published(ws, &remote)?;
                return Ok(Some(Mode::Online));
            }
            match settle_pending(ws, &mut link, tip.as_deref())? {
                Settled::Unresolved => {
                    return Err(Error::Unresolved(format!(
                        "an ownership attempt is unsettled and the push that would settle it did \
                         not complete ({}); it counts for nothing until `bead sync` can settle it",
                        link.last_error
                    ))
                    .into());
                }
                Settled::Moved => return Ok(None),
                Settled::Clear => {}
            }
            // whatever this command had in flight is settled one way or the other
            receipt = None;
            let mut tx = Tx::new(ws, actor)?;
            let answer = build(&mut tx, Mode::Online).map_err(Round::Build)?;
            if !tx.pending.is_empty() {
                append_events(ws, &tx.replica, &tx.pending).map_err(Error::from)?;
            }
            if tx.ownership.is_empty() {
                forget(&stale)?; // nothing to settle: this command went through
                return Ok(Some(answer));
            }
            let outcome = publish_attempt(ws, &mut link, tip.as_deref(), &tx.ownership, signature)?;
            match outcome {
                PushOutcome::Ok => {
                    // this command went through: the actor has moved on from those
                    forget(&stale)?;
                    published(ws, &remote)?;
                    Ok(Some(answer))
                }
                PushOutcome::Refused => Err(refused(&link).into()),
                // someone else got there first; decide again against the new head
                PushOutcome::Rejected => Ok(None),
                PushOutcome::Uncertain => {
                    // the attempt stays pending; keep its answer in case the push did land
                    receipt = Some((tx.ownership, answer));
                    Ok(None)
                }
            }
        });
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
    Err(Error::SyncConflict(format!(
        "the remote journal kept moving; try again{}",
        conflict_detail(&link)
    ))
    .into())
}

/// Writes the attempt (`events`) as pending, its receipt (`signature`) first, and publishes it on
/// `tip`. Accepted, it counts here at once; definitely refused or rejected, it is dropped;
/// uncertain, it stays pending for the next round or `sync` to settle. Lock held.
fn publish_attempt(
    ws: &Workspace,
    link: &mut Link<'_>,
    tip: Option<&str>,
    events: &[Value],
    signature: &str,
) -> Result<PushOutcome, Error> {
    let failpoints = &ws.settings.failpoints;
    let id = events
        .first()
        .and_then(|e| e.get("id"))
        .map(beadroll_core::text::str_of)
        .unwrap_or_default();
    let folder = ws.dir.join("pending");
    private_dirs(&folder)?;
    // the receipt first
    write_atomic(
        &folder.join(format!("{id}.command")),
        signature.as_bytes(),
        true,
        failpoints,
    )?;
    write_attempt_file(&folder.join(format!("{id}.json")), events, failpoints)?;
    failpoints.hit("ownership.before-publish");
    failpoints.hit("claim.before-push");
    let outcome = match link.publish(tip, "claim", events, false) {
        Ok(outcome) => outcome,
        Err(error) if error.code() == Code::RemoteUnreachable => PushOutcome::Uncertain,
        Err(error) => return Err(error),
    };
    let attempt = [events.to_vec()];
    match outcome {
        PushOutcome::Ok => {
            // the remote accepted; the acknowledgement may yet be lost
            failpoints.hit("ownership.after-publish-ok");
            confirm_ownership(ws, events)?;
            if let Some(Fired::Error(code)) = failpoints.hit("ownership.after-confirm") {
                // confirmed here, its pending twin not yet removed
                return Err(Error::injected(code, "ownership.after-confirm"));
            }
            drop_pending(ws, &attempt)?;
        }
        // it definitely did not land: refused, it will not on a retry either
        PushOutcome::Refused | PushOutcome::Rejected => drop_pending(ws, &attempt)?,
        PushOutcome::Uncertain => {}
    }
    Ok(outcome)
}

/// Why a round of [`coordinated`] stopped: the protocol's own failure, which one rebuild of a
/// damaged sync repository may cure, or the command's.
enum Round<E> {
    Sync(Error),
    Build(E),
}

impl<E> From<Error> for Round<E> {
    fn from(error: Error) -> Round<E> {
        Round::Sync(error)
    }
}

/// The remote has all of this store: the counts it holds, and nothing left unpublished.
fn published(ws: &Workspace, remote: &OsStr) -> Result<(), Error> {
    let counts = published_counts(ws)?;
    ws.note_sync(remote, &[("published", Value::Object(counts))]);
    ws.set_wrote(false);
    Ok(())
}
