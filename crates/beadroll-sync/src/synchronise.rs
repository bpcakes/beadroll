//! Exchanging journals with the coordination remote (Python's `synchronise`, `adopt_published`,
//! `publish_soon` and the unpublished counts).

use std::ffi::{OsStr, OsString};
use std::time::Duration;

use beadroll_core::json::{self, Format, Int, MAX_DIGITS, Object, Value};
use beadroll_core::text::py_int;
use beadroll_git::{PushOutcome, exists, realpath, without_password};
use beadroll_store::{
    Failpoints, Fired, Workspace, claim_files, load_pending, moved_workspaces, pending_files, read,
    relative, segment_files, view, workspaces_named, write_atomic,
};

use crate::error::Error;
use crate::link::{Link, newlines};
use crate::tx::{drop_pending, locked};

/// Rounds a sync or an ownership command makes before it gives up.
pub const ROUNDS: u32 = 12;

/// The wait between attempts against a moving remote: `U(0.1, 0.4) × attempt` seconds, outside
/// the store lock. The `backoff` failpoint passes here (`skip` waits not at all).
pub fn pause(attempt: u32, failpoints: &Failpoints) {
    if failpoints.hit("backoff") == Some(Fired::Skip) {
        return;
    }
    let seconds = (0.1 + fastrand::f64() * 0.3) * f64::from(attempt);
    std::thread::sleep(Duration::from_secs_f64(seconds));
}

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
        Value::Object(out)
    }
}

/// Exchanges journals with `remote`: fetch, merge, publish, in up to [`ROUNDS`] rounds while the
/// remote keeps moving. Pending attempts of this machine are settled on the way: a merge confirms
/// any the remote holds, and a fence commit on the head makes sure the rest can no longer land
/// before they are dropped. A damaged `sync.git` is rebuilt once. A URL-derived workspace that finds
/// this checkout's origin holding a tracker under another id adopts it (`moved.json`).
pub fn synchronise(ws: &Workspace, remote: &OsStr) -> Result<Synced, Error> {
    let mut link = Link::new(ws, remote)?;
    let known_renames = view(ws)?.renames().len();
    let (mut pulled, mut pushed) = (0i64, 0i64);
    let mut repacked = false;
    let mut settled = None;
    for attempt in 1..=ROUNDS {
        let round = sync_round(ws, &mut link, &mut pulled, &mut pushed, &mut repacked);
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
                return synchronise(&adopted, remote);
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
    let state = view(ws)?;
    let mut values = Vec::new();
    if let Some(name) = &ws.name {
        values.push(("name", Value::from(name.to_string_lossy().into_owned())));
    }
    values.push(("published", Value::Object(published_counts(ws)?)));
    ws.note_sync(remote, &values);
    ws.set_wrote(false);
    Ok(Synced {
        pulled,
        pushed,
        attempts,
        tip: link.last_commit.clone(),
        remote: remote.to_owned(),
        sync_repo_rebuilt: link.rebuilt.clone(),
        repacked,
        issues: state.issues().count(),
        renamed: state
            .renames()
            .iter()
            .skip(known_renames)
            .map(|r| Value::Object(r.to_value()))
            .collect(),
        forked_journals: link.forks.clone(),
        confirmed: link.confirmed.clone(),
        waiting_events: state.pending().len(),
        integrity: state.integrity().len(),
    })
}

/// `NOT_INITIALIZED` when neither this store nor `shown`, the remote, has a tracker: `bead init`
/// starts one, and a repository that moved without its tracker gets it back from a machine that
/// has it, published under its own id: this machine, when it holds a workspace of the same name.
fn nothing_published(ws: &Workspace, shown: &str) -> Error {
    let mut message = format!(
        "no tracker here and none published on {shown}: run `bead init` to start one. If this \
         repository moved and its tracker stayed behind, publish it here from a machine that has \
         it: `BEAD_WORKSPACE=ID bead sync`, ID as that machine's `bead info` shows it"
    );
    let named = match &ws.name {
        Some(name) => workspaces_named(&ws.settings, name, &ws.id).unwrap_or_default(),
        None => Vec::new(),
    };
    if !named.is_empty() {
        let commands: Vec<String> = named
            .iter()
            .map(|id| format!("`BEAD_WORKSPACE={id} bead sync`"))
            .collect();
        let name = ws.name.as_deref().unwrap_or_default().to_string_lossy();
        message.push_str(&format!(
            "; this machine has a tracker named {name}: {}",
            commands.join(" or ")
        ));
    }
    Error::NotInitialized(message)
}

/// One round: whether it went through (the remote took the publish).
fn sync_round(
    ws: &Workspace,
    link: &mut Link<'_>,
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
        *pulled += link.merge(tip.as_deref())?;
        if tip.is_none() && segment_files(ws)?.is_empty() {
            return Err(nothing_published(ws, &link.url_text()));
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
    remote: &OsStr,
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
    let origin = without_password(ws.origin.as_deref().unwrap_or_default()).0;
    if origin.is_empty() {
        return Ok(None);
    }
    let cwd = &ws.settings.cwd;
    let resolved = if exists(&origin, cwd) {
        realpath(&origin, cwd).into_os_string()
    } else {
        origin
    };
    if remote != resolved {
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
    let mut moved = moved_workspaces(&ws.settings)?;
    moved.insert(derived.clone(), Value::from(found.as_str()));
    let text = json::write(&Value::Object(moved), Format::INDENTED)
        .map_err(|e| Error::Validation(e.to_string()))?;
    write_atomic(
        &ws.settings.home()?.join("moved.json"),
        text.as_bytes(),
        true,
        &ws.settings.failpoints,
    )?;
    Ok(Some(adopted))
}

/// What the remote holds of this store the moment a publish went through: lines per journal
/// segment and one per attempt file, by path. Kept in `meta.json` so that the unpublished count
/// needs no network.
pub fn published_counts(ws: &Workspace) -> Result<Object, Error> {
    let mut counts = Object::new();
    for path in segment_files(ws)? {
        let lines = newlines(&read(&path)?) as i64;
        counts.insert(relative(ws, &path), Value::from(lines));
    }
    for path in claim_files(ws)? {
        counts.insert(relative(ws, &path), Value::from(1));
    }
    Ok(counts)
}

/// Events of this store the remote does not have, as of the last publish that went through;
/// `None` when there is no remote to publish to, or when the last publish kept no counts.
/// Everything is unpublished before the first publish. The arithmetic is Python's, exact at any
/// size. A count in `meta.json` that is not a number, or a total too long for Python to print, is
/// an IO error, where Python dies with a traceback (`docs/RUST-PORT.md` §5 row 26).
pub fn unpublished_count(ws: &Workspace) -> Result<Option<Int>, Error> {
    match ws.remote(None) {
        Ok(Some(remote)) if !remote.is_empty() => {}
        _ => return Ok(None),
    }
    let meta = ws.meta(false)?;
    let published = match meta.get("published") {
        Some(Value::Object(published)) => published.clone(),
        _ if meta.get("last_sync").is_some_and(Value::truthy) => return Ok(None),
        _ => Object::new(),
    };
    let unreadable = |what: String| {
        Error::Store(beadroll_store::Error::io_message(format!(
            "{}: {what}; repair it, or run `bead sync`",
            ws.dir.join("meta.json").display()
        )))
    };
    let zero = Int::from(0);
    let mut count = Int::from(0);
    for path in segment_files(ws)? {
        let lines = Int::from(newlines(&read(&path)?) as i64);
        let known = match published.get(&relative(ws, &path)) {
            None => Int::from(0),
            Some(value) => count_of(value).ok_or_else(|| {
                unreadable(format!(
                    "the published count of {} is not a number",
                    relative(ws, &path)
                ))
            })?,
        };
        let behind = lines.sub(&known);
        count = count.add(if behind > zero { &behind } else { &zero });
    }
    for path in claim_files(ws)? {
        if !published.contains_key(&relative(ws, &path)) {
            count = count.add(&Int::from(1));
        }
    }
    if count.digit_count() > MAX_DIGITS {
        return Err(unreadable(format!(
            "the published counts put the unpublished count beyond {MAX_DIGITS} digits"
        )));
    }
    Ok(Some(count))
}

/// Python's `int(value or 0)` of a published count: a falsy value is 0, an integer itself, a float
/// truncated, a string read as `int()` reads it; anything else is not a number.
fn count_of(value: &Value) -> Option<Int> {
    if !value.truthy() {
        return Some(Int::from(0));
    }
    match value {
        Value::Bool(true) => Some(Int::from(1)),
        Value::Int(n) => Some(n.clone()),
        Value::Float(f) => Int::from_f64(*f),
        Value::Str(text) => py_int(text),
        _ => None,
    }
}

/// Ownership ended or a local write happened: tell the other machines now rather than at the next
/// routine sync. A failure leaves the local write standing, and says so.
pub fn publish_soon(ws: &Workspace, offline: bool, remote: Option<&OsStr>) -> Option<String> {
    if offline {
        return None;
    }
    let outcome = ws
        .remote(remote)
        .map_err(Error::from)
        .and_then(|remote| match remote {
            None => Ok(()),
            Some(remote) if remote.is_empty() => Ok(()),
            Some(remote) => synchronise(ws, &remote).map(drop),
        });
    let error = outcome.err()?;
    Some(match error.os_error() {
        Some(said) => format!("recorded locally, not yet published (IO: {said})"),
        None => format!("recorded locally, not yet published ({})", error.code()),
    })
}
