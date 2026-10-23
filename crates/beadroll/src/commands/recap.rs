//! `bead recap`: what is new in every tracker on this machine since a recap last marked activity
//! seen (roll-dih8.2). It reads each store's journal and its projection's state as `log` does, one
//! store at a time, each let go of before the next, and never a checkout, git or the network. What
//! it shows is marked seen only once the answer has been written ([`Delivered`]).

mod layout;

use std::collections::BTreeMap;
use std::sync::Arc;

use beadroll_core::id::EventId;
use beadroll_core::json::{self, Format, Object, Value};
use beadroll_core::op::Op;
use beadroll_core::recap::{Window, joined, unseen};
use beadroll_core::state::{Integrity, Issue, State};
use beadroll_core::text::str_of;
use beadroll_core::time::event_time;
use beadroll_store::{
    Mark, RecapCursor, Settings, Workspace, home_stores, load_journal, machine_aliases,
    view_outline,
};

pub(crate) use self::layout::Layout;
use self::layout::{activity, time};
use super::history::logged;
use super::{Delivered, Failure, Line, Outcome, Tone};

/// How far back a machine's first recap reaches: a day, in milliseconds.
const FIRST: u64 = 86_400_000;

/// `recap`'s arguments, as the command line decided them.
pub(crate) struct RecapArgs {
    /// Every event, seen or not (`--all`).
    pub(crate) all: bool,
    /// Whether what it shows is marked seen once the answer is written: on a terminal, or when
    /// asked (`--advance`).
    pub(crate) advance: bool,
    pub(crate) layout: Layout,
    /// Now, in milliseconds since 1970: a machine's first recap shows the day before it.
    pub(crate) now: u64,
}

/// What is new in every tracker under `BEAD_HOME`: for each, the events not below the frontier the
/// cursor keeps of it, or, for one never shown, those since the window it is owed or else since
/// the last recap marked anything (the last day, on a machine that never did). With it, when the
/// recap marks what it shows as seen and that changes the cursor, what records that once the answer
/// has been written: each mark that changed (one with nothing new to show may still compact a
/// frontier), and this recap's time when it showed or counted an event, seen before or not. A
/// tracker that cannot be read says why and lets the others show; it keeps its frontier, or, never
/// shown, is owed its window from then on ([`owed`]).
pub(crate) fn recap(
    settings: &Arc<Settings>,
    args: &RecapArgs,
) -> Result<(Outcome, Option<Delivered>), Failure> {
    let cursor = RecapCursor::read(settings)?;
    let aliases = machine_aliases(settings);
    let first = cursor.at.unwrap_or(args.now.saturating_sub(FIRST));
    let (mut trackers, mut lines, mut marks) = (Vec::new(), Vec::new(), BTreeMap::new());
    let mut seen = false;
    for found in home_stores(settings)? {
        let part = match found {
            Ok(ws) => {
                let mark = cursor.marks.get(&ws.id);
                let window = match (args.all, mark) {
                    (true, _) => Window::All,
                    (false, Some(Mark::Shown(frontier))) => Window::After(frontier),
                    (false, Some(Mark::Owed(since))) => Window::Since(*since),
                    (false, None) => Window::Since(first),
                };
                let kept = match mark {
                    Some(Mark::Shown(frontier)) => frontier.as_slice(),
                    Some(Mark::Owed(_)) | None => &[],
                };
                let owed = owed(mark, first);
                tracker(&ws, window, kept, owed, &aliases, args.layout)
            }
            Err(unusable) => {
                let id = unusable.id.as_deref();
                let mark = id.and_then(|id| owed(cursor.marks.get(id), first));
                let mark = mark
                    .zip(id)
                    .map(|(since, id)| (id.to_owned(), Mark::Owed(since)));
                let failure = Failure::from(unusable.error);
                Part::unusable(&unusable.name, None, Value::Null, &failure, mark)
            }
        };
        trackers.push(Value::from(part.value));
        lines.extend(part.lines);
        // what it shows marks the time, events seen before (`--all`) as much as new ones, whether
        // its mark changes or not
        seen |= part.seen;
        if let Some((id, mark)) = part.mark
            && cursor.marks.get(&id) != Some(&mark)
        {
            marks.insert(id, (mark, part.placed));
        }
    }
    if trackers.is_empty() {
        lines.push("No local trackers".into());
    }
    let mut result = Object::new();
    result.insert("workspaces".into(), Value::Array(trackers));
    result.insert("advance".into(), Value::from(args.advance));
    let delivered = (args.advance && (seen || !marks.is_empty())).then(|| {
        // a window owed or a frontier compacted, with nothing shown or counted, marks nothing
        // seen: the time of the last recap that showed or counted an event stays
        let (settings, at) = (Arc::clone(settings), seen.then_some(args.now));
        Delivered::new(move || Ok(cursor.save(&settings, marks, at)?))
    });
    Ok((Outcome::new(result, lines), delivered))
}

/// The window a tracker that cannot be read is owed from then on, in milliseconds since 1970: the
/// one it is owed already, or, when it has never been shown, where a tracker first seen now starts
/// (`first`). None once it has been shown: its frontier stays as it is. Without it, a tracker found
/// unreadable would start at the next recap's time instead, and what came before that would be
/// counted as earlier without ever being shown.
fn owed(mark: Option<&Mark>, first: u64) -> Option<u64> {
    match mark {
        Some(Mark::Shown(_)) => None,
        Some(Mark::Owed(since)) => Some(*since),
        None => Some(first),
    }
}

/// One tracker's part of the answer: its JSON, its lines, and its id and the mark the cursor is to
/// keep of it: its frontier once shown, the window it is owed until a recap can show or count any
/// of its events, or none.
struct Part {
    value: Object,
    lines: Vec<Line>,
    mark: Option<(String, Mark)>,
    /// Every event its read placed, sorted: below its frontier, so that a save over a frontier
    /// another recap saved meanwhile keeps only what this read did not place.
    placed: Vec<EventId>,
    /// Whether it showed or counted an event: what marks activity seen.
    seen: bool,
}

impl Part {
    /// A tracker that cannot be read: its id (or its directory's name), its name and last sync as
    /// far as they are known, and why; with the window it is owed meanwhile, when it is.
    fn unusable(
        workspace: &str,
        name: Option<Value>,
        synced: Value,
        failure: &Failure,
        mark: Option<(String, Mark)>,
    ) -> Part {
        let shown = name.as_ref().map(str_of);
        let mut error = Object::new();
        error.insert("code".into(), Value::from(failure.code.as_str()));
        error.insert("message".into(), Value::from(failure.message.as_str()));
        let mut value = Object::new();
        value.insert("workspace".into(), Value::from(workspace));
        value.insert("name".into(), name.unwrap_or(Value::Null));
        value.insert("last_sync".into(), synced);
        value.insert("error".into(), Value::from(error));
        let lines = vec![
            heading(shown.as_deref(), workspace),
            Line::toned(
                Tone::Error,
                format!("  cannot be read: {}", failure.printed()),
            ),
        ];
        Part {
            value,
            lines,
            mark,
            placed: Vec::new(),
            seen: false,
        }
    }
}

/// The line a tracker's part starts with: its name, and its id when that is not its name.
fn heading(name: Option<&str>, workspace: &str) -> Line {
    match name.filter(|name| *name != workspace) {
        Some(name) => Line::toned(Tone::Heading, name).with(Tone::Dim, format!(" [{workspace}]")),
        None => Line::toned(Tone::Heading, workspace),
    }
}

/// What `ws` shows through `window`, laid out as `layout`: its journal and its state read as `log`
/// reads them, the state after the journal so that every issue an event names is in it, and both
/// let go of when it returns. Its name is the repository's as this machine last saw it in a
/// checkout, else the tracker's own; a `meta.json` that cannot be read is no reason to stop here.
/// With the mark the cursor is to keep of it, given `kept`, the frontier the cursor holds of it,
/// and the window it is `owed` when it has never been shown.
fn tracker(
    ws: &Workspace,
    window: Window<'_>,
    kept: &[EventId],
    owed: Option<u64>,
    aliases: &BTreeMap<String, String>,
    layout: Layout,
) -> Part {
    let meta = ws.metadata(false).unwrap_or_default();
    let synced = meta.value("last_sync").cloned().unwrap_or(Value::Null);
    let named = meta.value("name").filter(|name| name.truthy()).cloned();
    let read = load_journal(ws)
        .map_err(Failure::from)
        .and_then(|journal| Ok((view_outline(ws)?, journal)));
    let (st, journal) = match read {
        Ok(read) => read,
        Err(failure) => {
            let mark = owed.map(|since| (ws.id.clone(), Mark::Owed(since)));
            return Part::unusable(&ws.id, named, synced, &failure, mark);
        }
    };
    let name = (named.or_else(|| st.name().filter(|name| name.truthy()).cloned()))
        .map_or_else(|| ws.id.clone(), |name| str_of(&name));
    let shown = unseen(&journal, window);
    let mut rows = Vec::with_capacity(shown.events.len());
    for event in shown.events.iter().filter_map(|id| journal.event(*id)) {
        let title = (event.issue().and_then(|uid| st.issue_named(uid)))
            .map(Issue::title)
            .filter(|title| !title.is_empty());
        let (mut row, line) = logged(event, &st, aliases, title);
        row.insert("machine_id".into(), Value::from(event.replica()));
        row.insert("title".into(), title.map_or(Value::Null, Value::from));
        row.insert("summary".into(), Value::from(summary(event.op(), &st)));
        let commit = (event.place().filter(|place| place.truthy()))
            .and_then(|place| place.get("commit"))
            .cloned();
        row.insert("commit".into(), commit.unwrap_or(Value::Null));
        rows.push((event, row, line));
    }
    let mut lines = opening(&name, &ws.id, &synced, window, shown.earlier, &st);
    lines.extend(activity(layout, &rows, &st));
    let mut value = Object::new();
    value.insert("workspace".into(), Value::from(ws.id.as_str()));
    value.insert("name".into(), Value::from(name));
    value.insert("last_sync".into(), synced);
    let since = match window {
        Window::Since(millis) => Value::from(event_time(millis)),
        Window::After(_) | Window::All => Value::Null,
    };
    value.insert("since".into(), since);
    value.insert("earlier".into(), number(shown.earlier));
    let events = rows.into_iter().map(|(_, row, _)| Value::from(row));
    value.insert("events".into(), Value::Array(events.collect()));
    value.insert("pending".into(), number(st.pending().len() as u64));
    let integrity = st.integrity().iter().map(|row| Value::from(row.to_value()));
    value.insert("integrity".into(), Value::Array(integrity.collect()));
    // the window applies to what a recap shows or counts: a tracker never shown, of which this one
    // did neither (nothing but its init in causal order yet), stays owed it; else the cursor keeps
    // the frontier, with what of the one it held this read did not place, so that a recap with
    // nothing new replaces the frontiers two recaps at once left with its own
    let seen = !shown.events.is_empty() || shown.earlier > 0;
    let mark = match owed {
        Some(since) if !seen => Mark::Owed(since),
        _ => Mark::Shown(joined(&shown.frontier, &shown.placed, kept)),
    };
    Part {
        value,
        lines,
        mark: Some((ws.id.clone(), mark)),
        placed: shown.placed,
        seen,
    }
}

/// The lines a readable tracker's part opens with: its heading and its last sync; where a first
/// recap's window starts, and how many `earlier` events it left out; and, from `st`, each of its
/// integrity problems and how many events wait for causes.
fn opening(
    name: &str,
    workspace: &str,
    synced: &Value,
    window: Window<'_>,
    earlier: u64,
    st: &State,
) -> Vec<Line> {
    let synced = if synced.truthy() {
        str_of(synced)
    } else {
        "never".to_owned()
    };
    let heading = heading(Some(name), workspace).with(Tone::Dim, format!("  last sync {synced}"));
    let mut lines = vec![heading];
    if let Window::Since(millis) = window {
        let earlier = match earlier {
            0 => String::new(),
            1 => "; 1 earlier event not shown (--all shows it)".to_owned(),
            n => format!("; {n} earlier events not shown (--all shows them)"),
        };
        let since = time(&event_time(millis));
        let line = format!("  first seen here: since {since}{earlier}");
        lines.push(Line::toned(Tone::Dim, line));
    }
    let count = |n: usize, what: &str| format!("{n} {what}{}", if n == 1 { "" } else { "s" });
    if !st.integrity().is_empty() {
        let problems = count(st.integrity().len(), "integrity problem");
        lines.push(Line::toned(Tone::Warning, format!("  {problems}:")));
        let each = st
            .integrity()
            .iter()
            .map(|row| format!("    {}", problem(row)));
        lines.extend(each.map(|line| Line::toned(Tone::Warning, line)));
    }
    if !st.pending().is_empty() {
        let waiting = count(st.pending().len(), "event");
        let line = format!("  {waiting} waiting for causes not here yet: shown once they arrive");
        lines.push(Line::toned(Tone::Warning, line));
    }
    lines
}

/// An integrity problem as `bead info` gives it, in a line: what it is about (the issue, else the
/// event), what is wrong, and where the record was found, when it was a record that could not be
/// used.
fn problem(row: &Integrity) -> String {
    let about = row.issue.as_deref().or(row.event.as_deref());
    match (about, row.origin.as_deref()) {
        (Some(about), Some(origin)) => format!("{about}: {} ({origin})", row.problem),
        (Some(about), None) => format!("{about}: {}", row.problem),
        (None, Some(origin)) => format!("{origin}: {}", row.problem),
        (None, None) => row.problem.clone(),
    }
}

/// `n` as JSON.
fn number(n: u64) -> Value {
    Value::from(i64::try_from(n).unwrap_or(i64::MAX))
}

/// What an event did, in a few words, as it was recorded: not what its issue is now.
fn summary(op: &Op, st: &State) -> String {
    let why = |what: &str, reason: Option<&str>| match reason.filter(|r| !r.is_empty()) {
        Some(reason) => format!("{what}: {reason}"),
        None => what.to_owned(),
    };
    match op {
        Op::Create(create) if create.imported => "create (imported)".to_owned(),
        Op::Set(fields) => {
            let fields: Vec<String> = (fields.iter())
                .map(|(field, value)| {
                    let value =
                        json::write(value, Format::SPACED).unwrap_or_else(|_| str_of(value));
                    format!("{}={value}", field.name())
                })
                .collect();
            format!("set {}", fields.join(", "))
        }
        Op::Comment { text, .. } => format!("comment: {text}"),
        Op::Claim(claim) => {
            let what = if claim.mode.as_deref() == Some("offline") {
                "claim (provisional)"
            } else if claim.takeover.as_deref().is_some_and(|t| !t.is_empty()) {
                "takeover"
            } else {
                "claim"
            };
            why(what, claim.reason.as_deref())
        }
        Op::Release(ending) | Op::Close(ending) | Op::Delete(ending) => {
            why(op.name(), ending.reason.as_deref())
        }
        Op::DepAdd { to, kind, .. } => {
            let kind = kind
                .as_deref()
                .filter(|k| !k.is_empty())
                .unwrap_or("blocks");
            format!("dep_add {kind} {}", st.key_of(to))
        }
        Op::DepRemove { to, key, kind } => {
            let to = to.as_deref().or(key.as_deref()).unwrap_or("?");
            let kind = kind.as_deref().unwrap_or("every type");
            format!("dep_remove {kind} {}", st.key_of(to))
        }
        Op::LabelAdd(labels) | Op::LabelRemove(labels) | Op::LabelSet(labels) => {
            format!("{} {}", op.name(), labels.join(", "))
        }
        _ => op.name().to_owned(),
    }
}
