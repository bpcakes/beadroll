//! Journal exchange, published-tracker adoption, immediate publication and unpublished counts.

use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::time::Duration;

use beadroll_core::json::{self, Format, Int, Object, Value};
use beadroll_core::text::{parse_integer, shell_quote};
use beadroll_git::PushOutcome;
use beadroll_store::{
    Endpoint, Failpoints, Fired, Memo, Workspace, claim_files, load_pending, moved_workspaces,
    pending_files, relative, relative_name, segment_files, workspaces_named, write_atomic,
};

use crate::error::Error;
use crate::link::Link;
use crate::tx::{drop_pending, locked};

/// Maximum sync or ownership rounds before giving up, allowing retries under concurrent writers
/// (roll-lqjj.7).
pub const ROUNDS: u32 = 24;

/// The longest wait between two rounds, in seconds.
const LONGEST_PAUSE: f64 = 2.0;

/// Random backoff outside the store lock, from zero up to `0.1 × 2^(attempt - 1)` seconds, capped
/// at [`LONGEST_PAUSE`]. Jitter separates competing writers. The backoff failpoint can skip the
/// wait.
pub fn pause(attempt: u32, failpoints: &Failpoints) {
    if failpoints.hit("backoff") == Some(Fired::Skip) {
        return;
    }
    let seconds = fastrand::f64() * longest_pause(attempt);
    std::thread::sleep(Duration::from_secs_f64(seconds));
}

/// The bound is separate from sampling and sleeping so its schedule can be checked without waits.
fn longest_pause(attempt: u32) -> f64 {
    let doubled = f64::from(1u32 << attempt.saturating_sub(1).min(8));
    (0.1 * doubled).min(LONGEST_PAUSE)
}

#[cfg(test)]
mod tests;

/// What the remote last answered, when it was not the plain race the message names.
pub fn conflict_detail(link: &Link<'_>) -> String {
    if link.last_error.contains("[remote rejected]") {
        format!(" (the remote last answered: {})", link.last_error)
    } else {
        String::new()
    }
}

/// The remote refuses bead's update of its ref: retrying will not help.
pub fn refused(link: &Link<'_>) -> Error {
    Error::RemoteRefused(format!(
        "the remote refuses bead's update of {}; nothing was lost, and retrying will not help \
         until the remote allows it: {}",
        link.ref_name, link.last_error
    ))
}

/// What a sync did, in the order `bead sync --json` reports it.
#[derive(Clone, Debug, PartialEq)]
pub struct Synced {
    /// Events that came in.
    pub pulled: i64,
    /// Events the remote lacked before the push.
    pub pushed: i64,
    /// Rounds it took.
    pub attempts: u32,
    /// The commit the remote holds now.
    pub tip: Option<String>,
    /// The remote.
    pub remote: OsString,
    /// Where a damaged `sync.git` was set aside.
    pub sync_repo_rebuilt: Option<String>,
    /// Whether `sync.git` was repacked.
    pub repacked: bool,
    /// Issues now.
    pub issues: usize,
    /// Keys renamed since the sync began.
    pub renamed: Vec<Value>,
    /// Journal files whose local copy differed from the published one.
    pub forked_journals: Vec<String>,
    /// This machine's pending attempts found accepted.
    pub confirmed: Vec<String>,
    /// Events waiting for a cause.
    pub waiting_events: usize,
    /// Integrity problems.
    pub integrity: usize,
}

impl Synced {
    /// The result as `bead sync --json` prints it.
    pub fn to_value(&self) -> Value {
        let mut out = Object::new();
        out.insert("pulled".into(), Value::from(self.pulled));
        out.insert("pushed".into(), Value::from(self.pushed));
        out.insert("attempts".into(), Value::from(i64::from(self.attempts)));
        out.insert("tip".into(), Value::from(self.tip.clone()));
        out.insert(
            "remote".into(),
            Value::from(self.remote.to_string_lossy().into_owned()),
        );
        if let Some(rebuilt) = &self.sync_repo_rebuilt {
            out.insert("sync_repo_rebuilt".into(), Value::from(rebuilt.as_str()));
        }
        if self.repacked {
            out.insert("repacked".into(), Value::Bool(true));
        }
        out.insert("issues".into(), Value::from(self.issues as i64));
        out.insert("renamed".into(), Value::Array(self.renamed.clone()));
        let texts = |items: &[String]| {
            Value::Array(items.iter().map(|s| Value::from(s.as_str())).collect())
        };
        out.insert("forked_journals".into(), texts(&self.forked_journals));
        out.insert("confirmed".into(), texts(&self.confirmed));
        out.insert(
            "waiting_events".into(),
            Value::from(self.waiting_events as i64),
        );
        out.insert("integrity".into(), Value::from(self.integrity as i64));
        Value::from(out)
    }
}

/// Exchanges journals with `remote`: fetch, merge, publish, in up to [`ROUNDS`] rounds while the
/// remote keeps moving. Pending attempts of this machine are settled on the way: a merge confirms
/// any the remote holds, and a fence commit on the head makes sure the rest can no longer land
/// before they are dropped. A damaged `sync.git` is rebuilt once. A URL-derived workspace that finds
/// this checkout's origin holding a tracker under another id adopts it (`moved.json`).
pub fn synchronise(ws: &Workspace, remote: &Endpoint) -> Result<Synced, Error> {
    exchange(ws, remote, true)?.ok_or_else(|| Error::Validation("no report".to_owned()))
}

/// [`synchronise`], and what it did when `report` asks for it: a write published at once is not
/// told. The store's state is read once per round, by its merge, and the report is made of the
/// state the last merge left (roll-lqjj.29); the keys renamed before the sync are counted by the
/// first merge, from the projection it reads before it merges, which is the one it goes on with
/// when nothing comes in.
fn exchange(ws: &Workspace, remote: &Endpoint, report: bool) -> Result<Option<Synced>, Error> {
    let mut link = Link::new(ws, remote)?;
    let (mut pulled, mut pushed) = (0i64, 0i64);
    let mut repacked = false;
    let mut settled = None;
    for attempt in 1..=ROUNDS {
        let round = sync_round(
            ws,
            &mut link,
            report,
            &mut pulled,
            &mut pushed,
            &mut repacked,
        );
        match round {
            Ok(true) => {
                settled = Some(attempt);
                break;
            }
            Ok(false) => {}
            Err(error) => {
                if matches!(error, Error::SyncRepoDamaged(_)) && link.rebuilt.is_none() {
                    locked(ws, || link.rebuild())?;
                    continue;
                }
                let adopted = if attempt == 1 {
                    adopt_published(ws, remote, &error)?
                } else {
                    None
                };
                let Some(adopted) = adopted else {
                    return Err(error);
                };
                return exchange(&adopted, remote, report);
            }
        }
        pause(attempt, &ws.settings.failpoints);
    }
    let Some(attempts) = settled else {
        return Err(Error::SyncConflict(format!(
            "the remote journal kept moving; run sync again{}",
            conflict_detail(&link)
        )));
    };
    let mut values = Vec::new();
    if let Some(name) = &ws.name {
        values.push(("name", Value::from(name.to_string_lossy().into_owned())));
    }
    values.push(("published", Value::from(link.published_counts()?)));
    ws.note_sync(remote, &values);
    ws.set_wrote(false);
    if !report {
        return Ok(None);
    }
    let Some(merged) = link.merged.take() else {
        return Err(Error::Validation("no state".to_owned()));
    };
    let state = merged.state();
    Ok(Some(Synced {
        pulled,
        pushed,
        attempts,
        tip: link.last_commit.clone(),
        remote: remote.url.clone(),
        sync_repo_rebuilt: link.rebuilt.clone(),
        repacked,
        issues: state.issues().count(),
        renamed: state
            .renames()
            .iter()
            .skip(link.renames_before.unwrap_or_default())
            .map(|r| Value::from(r.to_value()))
            .collect(),
        forked_journals: link.forks.clone(),
        confirmed: link.confirmed.clone(),
        waiting_events: state.pending().len(),
        integrity: state.integrity().len(),
    }))
}

/// `NOT_INITIALIZED` when neither this store nor `remote` has a tracker: `bead init` starts one, and
/// a repository that moved without its tracker gets it back from a machine that has it, published
/// under its own id to `remote` given on the command line as it was chosen ([`Endpoint::chosen`]),
/// where that tracker would not otherwise go: this machine, when it holds a workspace of the same
/// name.
fn nothing_published(ws: &Workspace, remote: &Endpoint) -> Error {
    let shown = remote.url.to_string_lossy();
    let sync = format!(
        "bead sync --remote {}",
        shell_quote(&remote.chosen().to_string_lossy())
    );
    let mut message = format!(
        "no tracker here and none published on {shown}: run `bead init` to start one. If this \
         repository moved and its tracker stayed behind, publish it here from a machine that has \
         it: `BEAD_WORKSPACE=ID {sync}`, ID as that machine's `bead info` shows it"
    );
    let named = match &ws.name {
        Some(name) => workspaces_named(&ws.settings, name, &ws.id).unwrap_or_default(),
        None => Vec::new(),
    };
    if !named.is_empty() {
        let commands: Vec<String> = named
            .iter()
            .map(|id| format!("`BEAD_WORKSPACE={id} {sync}`"))
            .collect();
        let name = ws.name.as_deref().unwrap_or_default().to_string_lossy();
        message.push_str(&format!(
            "; this machine has a tracker named {name}: {}",
            commands.join(" or ")
        ));
    }
    Error::NotInitialized(message)
}

/// One round: whether it went through (the remote took the publish). With `report`, the first merge
/// counts the keys renamed before it ([`Link::merge_from`]).
fn sync_round(
    ws: &Workspace,
    link: &mut Link<'_>,
    report: bool,
    pulled: &mut i64,
    pushed: &mut i64,
    repacked: &mut bool,
) -> Result<bool, Error> {
    let mut tip = link.fetch()?;
    locked(ws, || {
        if !load_pending(ws)?.is_empty() {
            // also sweeps pending files that are not attempts; settling our own attempts needs the
            // current head, not a snapshot
            tip = link.fetch()?;
        }
        *pulled += link.merge_from(tip.as_deref(), report)?;
        if tip.is_none() && segment_files(ws)?.is_empty() {
            return Err(nothing_published(ws, &link.endpoint()));
        }
        *pushed = link.unpublished()?;
        let fence = !load_pending(ws)?.is_empty();
        match link.publish(tip.as_deref(), "sync", &[], fence)? {
            PushOutcome::Ok => {
                // absent at a head we just advanced: they can never land
                drop_pending(ws, &load_pending(ws)?)?;
                *repacked = link.maintain()?;
                Ok(true)
            }
            PushOutcome::Refused => Err(refused(link)),
            PushOutcome::Uncertain => Err(Error::RemoteUnreachable(format!(
                "the push did not complete; nothing was lost, run sync again once the cause is \
                 fixed: {}",
                link.last_error
            ))),
            PushOutcome::Rejected => Ok(false), // another machine pushed first: merge again
        }
    })
}

/// A URL-derived id that finds this checkout's own origin already holding a tracker under another
/// id (the repository was renamed, or its URL is spelled differently here) adopts that tracker,
/// once, while nothing has been written under the derived id. A remote named any other way is
/// never adopted.
pub fn adopt_published(
    ws: &Workspace,
    remote: &Endpoint,
    error: &Error,
) -> Result<Option<Workspace>, Error> {
    let Error::WorkspaceMismatch {
        found: Value::Str(found),
        ..
    } = error
    else {
        return Ok(None);
    };
    let Some(derived) = &ws.derived else {
        return Ok(None);
    };
    if ws.id != *derived {
        return Ok(None);
    }
    // this checkout's origin, chosen by its name or given as its very URL
    let origin = match &remote.name {
        Some(name) => name == "origin",
        None => ws.is_origin_url(&remote.url, false),
    };
    if !origin {
        return Ok(None);
    }
    if !segment_files(ws)?.is_empty()
        || !claim_files(ws)?.is_empty()
        || !pending_files(ws).is_empty()
    {
        return Ok(None);
    }
    let Ok(mut adopted) = Workspace::new(
        std::sync::Arc::clone(&ws.settings),
        found,
        ws.name.clone(),
        ws.origin.clone(),
        ws.config.clone(),
    ) else {
        return Ok(None);
    };
    adopted.derived = Some(derived.clone());
    adopted.origin_push = ws.origin_push;
    let mut moved = moved_workspaces(&ws.settings)?;
    moved.insert(derived.clone(), Value::from(found.as_str()));
    let text = json::write(&Value::from(moved), Format::INDENTED)
        .map_err(|e| Error::Validation(e.to_string()))?;
    write_atomic(
        &ws.settings.home()?.join("moved.json"),
        text.as_bytes(),
        true,
        &ws.settings.failpoints,
    )?;
    Ok(Some(adopted))
}

/// Events not covered by the last successful publication. Returns None without a remote or
/// without saved counts after publication; before the first publication, all events are
/// unpublished. Arithmetic is exact. Nonnumeric counts or a total exceeding the decimal output
/// limit return IO. Memoized line counts avoid rereading unchanged segments on each command.
pub fn unpublished_count(ws: &Workspace) -> Result<Option<Int>, Error> {
    match ws.remote(None) {
        Ok(Some(remote)) if !remote.is_empty() => {}
        _ => return Ok(None),
    }
    let meta = ws.meta(false)?;
    let none = Object::new();
    let published = match meta.get("published") {
        Some(Value::Object(published)) => &**published,
        _ if meta.get("last_sync").is_some_and(Value::truthy) => return Ok(None),
        _ => &none,
    };
    let unreadable = |what: String| {
        Error::Store(beadroll_store::Error::io_message(format!(
            "{}: {what}; repair it, or run `bead sync`",
            ws.dir.join("meta.json").display()
        )))
    };
    let zero = Int::from(0);
    let mut count = Int::from(0);
    let segments: Vec<(PathBuf, String)> = (segment_files(ws)?.into_iter())
        .map(|path| {
            let name = relative(ws, &path);
            (path, name)
        })
        .collect();
    // the segments' lines alone: what the memo says of each attempt file is not needed here, and
    // a segment's name ends as no attempt file's does, so that the lookup is for those alone
    let names: HashSet<&str> = segments.iter().map(|(_, name)| name.as_str()).collect();
    let mut memo = Memo::load_only(ws, &|name| name.ends_with(".jsonl") && names.contains(name));
    for (path, name) in &segments {
        let lines = Int::from(memo.lines(name, path)? as i64);
        let known = match published.get(name) {
            None => Int::from(0),
            Some(value) => count_of(value).ok_or_else(|| {
                unreadable(format!("the published count of {name} is not a number"))
            })?,
        };
        let beyond = || {
            unreadable(
                "the published counts put the unpublished count beyond what bead counts".to_owned(),
            )
        };
        let behind = lines.sub(&known).ok_or_else(beyond)?;
        count = count
            .add(if behind > zero { &behind } else { &zero })
            .ok_or_else(beyond)?;
    }
    for path in claim_files(ws)?.iter() {
        // its path in the store, as `relative` gives it, without a copy of each
        if !published.contains_key(&*relative_name(ws, path)) {
            count = count.add(&Int::from(1)).ok_or_else(|| {
                unreadable(
                    "the attempt files put the unpublished count beyond what bead counts"
                        .to_owned(),
                )
            })?;
        }
    }
    Ok(Some(count))
}

/// Coerces a published count: falsy values become zero, integers stay exact, finite floats
/// truncate toward zero and strings use integer parsing. Other values fail.
fn count_of(value: &Value) -> Option<Int> {
    if !value.truthy() {
        return Some(Int::from(0));
    }
    match value {
        Value::Bool(true) => Some(Int::from(1)),
        Value::Int(n) => Some(n.clone()),
        Value::Float(f) => Int::from_f64(*f),
        Value::Str(text) => parse_integer(text),
        _ => None,
    }
}

/// Ownership ended or a local write happened: tell the other machines now rather than at the next
/// routine sync. A failure leaves the local write standing, and says so.
pub fn publish_soon(ws: &Workspace, offline: bool, remote: Option<&OsStr>) -> Option<String> {
    publish_soon_at(ws, destination(ws, offline, remote))
}

/// Where [`publish_soon`] would publish: nowhere (`None`) when `offline`, or when the tracker has
/// no remote to publish to; else the remote, or why it could not be chosen. A command line chooses
/// it before it lets go of what a write held, so that a write with nowhere to go keeps that to the
/// end of the process as an offline write does (roll-lqjj.50). Choosing asks git what choosing
/// asked it when the publish chose: a remote's URL by its name, the rules that rewrite a local
/// path, the top of the checkout for a relative one, how an older version saved a remembered
/// remote; nothing more.
pub fn destination(
    ws: &Workspace,
    offline: bool,
    remote: Option<&OsStr>,
) -> Option<Result<Endpoint, Error>> {
    if offline {
        return None;
    }
    match ws.endpoint(remote) {
        Ok(Some(remote)) if !remote.url.is_empty() => Some(Ok(remote)),
        Ok(_) => None,
        Err(error) => Some(Err(error.into())),
    }
}

/// [`publish_soon`] to the [`destination`] chosen already.
pub fn publish_soon_at(
    ws: &Workspace,
    destination: Option<Result<Endpoint, Error>>,
) -> Option<String> {
    let outcome = match destination {
        None => Ok(()),
        Some(Ok(remote)) => publish_to(ws, &remote),
        Some(Err(error)) => Err(error),
    };
    not_yet_published(outcome)
}

/// [`publish_soon`] to `remote`, chosen already.
pub fn publish_soon_to(ws: &Workspace, remote: &Endpoint) -> Option<String> {
    not_yet_published(publish_to(ws, remote))
}

/// A sync with `remote`, when there is one to publish to.
fn publish_to(ws: &Workspace, remote: &Endpoint) -> Result<(), Error> {
    if remote.url.is_empty() {
        return Ok(());
    }
    exchange(ws, remote, false).map(drop)
}

/// What a failed publish says: the local write stands, not yet published.
fn not_yet_published(outcome: Result<(), Error>) -> Option<String> {
    let error = outcome.err()?;
    Some(match error.os_error() {
        Some(said) => format!("recorded locally, not yet published (IO: {said})"),
        None => format!("recorded locally, not yet published ({})", error.code()),
    })
}
