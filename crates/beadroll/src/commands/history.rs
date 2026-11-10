//! `bead log` and `bead machines`: who did what, and the machines that did it.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt as _;

use beadroll_core::code::Code;
use beadroll_core::history::HistoryEntry;
use beadroll_core::journal::Event;
use beadroll_core::json::{self, Format, Int, Object, Value};
use beadroll_core::state::State;
use beadroll_core::text::{printed_text, str_of};
use beadroll_core::time::event_time;
use beadroll_store::FileMode;
use beadroll_store::{
    Unreadable, history, machine_aliases, projection, read_json_text, view_outline, write_atomic,
};

use super::{Context, Failure, Line, Outcome, Tone, resolve_arg};

/// The tracker's events in causal order, excluding `init`, or one issue's events: time, machine,
/// who (the actor's name when it was given one, as its claims are labelled, else its agent and
/// thread), operation (with the fields a `set` changed) and issue. A positive `limit` keeps the
/// last that many; a negative limit leaves that many off the start.
///
/// The metadata and order come from the journal or its validated history cache. Issue keys
/// come from the state, read afterwards so every issue an event names is there (roll-lqjj.31).
/// A cache miss reads the whole journal; a hit checks source identities and reads only metadata.
/// Replay is needed only to rebuild a stale projection, as for every command.
pub(crate) fn log(
    ctx: &Context<'_>,
    key: Option<&OsStr>,
    limit: Option<&Int>,
) -> Result<Outcome, Failure> {
    let history = history(ctx.ws)?;
    let st = view_outline(ctx.ws)?;
    let aliases = machine_aliases(&ctx.ws.settings);
    let wanted = match key.filter(|k| !k.is_empty()) {
        Some(key) => Some(resolve_arg(&st, key)?.uid().to_string()),
        None => None,
    };
    let mut rows: Vec<_> = history
        .iter()
        .filter(|e| e.operation != "init")
        .filter(|e| wanted.is_none() || e.issue.as_deref() == wanted.as_deref())
        .collect();
    let limit = limit.map_or(0, Int::as_i128);
    if limit != 0 {
        let len = rows.len() as i128;
        let start = if limit > 0 {
            (len - limit).max(0)
        } else {
            limit.saturating_neg().min(len)
        };
        rows.drain(..usize::try_from(start).unwrap_or(0));
    }
    let mut result = Vec::new();
    let mut lines = Vec::new();
    for event in rows {
        let (row, line) = logged_summary(event, &st, &aliases, None);
        lines.push(line);
        result.push(Value::from(row));
    }
    if lines.is_empty() {
        lines.push("No events".into());
    }
    Ok(Outcome::new(Value::Array(result), lines))
}

/// One event as `log` shows it: its row, and its line, `title` after the issue when given (as
/// `recap` gives it): time, machine (its local display name, else its id), who (the actor's name,
/// else its agent and thread), operation (with the fields a `set` changed), issue and branch. The
/// keys are `st`'s, the state read after the journal the event is from.
pub(crate) fn logged(
    event: &Event,
    st: &State,
    aliases: &BTreeMap<String, String>,
    title: Option<&str>,
) -> (Object, Line) {
    logged_summary(&HistoryEntry::from(event), st, aliases, title)
}

fn logged_summary(
    event: &HistoryEntry,
    st: &State,
    aliases: &BTreeMap<String, String>,
    title: Option<&str>,
) -> (Object, Line) {
    let actor = |key: &str| match key {
        "agent" => event.agent.clone(),
        "thread" => event.thread.clone(),
        "name" => event.name.clone(),
        _ => Value::Null,
    };
    let at = event_time(event.id.millis());
    let replica = event.machine.as_str();
    // a machine whose name was taken back (`--name ID=`) goes by its id, as one never named
    let machine =
        (aliases.get(replica).filter(|alias| !alias.is_empty())).map_or(replica, String::as_str);
    let issue = match event.issue.as_deref() {
        Some(uid) => Value::from(st.key_of(uid)),
        None => Value::Null,
    };
    let branch = event.branch.clone();
    let mut row = Object::new();
    row.insert("at".into(), Value::from(at.as_str()));
    row.insert("machine".into(), Value::from(machine));
    row.insert("agent".into(), actor("agent"));
    row.insert("thread".into(), actor("thread"));
    row.insert("name".into(), actor("name"));
    row.insert("op".into(), Value::from(event.operation.as_str()));
    row.insert("issue".into(), issue.clone());
    row.insert("event".into(), Value::from(event.id.to_string()));
    row.insert("branch".into(), branch.clone());
    let or = |value: Value, default: &str| {
        if value.truthy() {
            str_of(&value)
        } else {
            default.to_owned()
        }
    };
    let who = if actor("name").truthy() {
        str_of(&actor("name"))
    } else {
        let thread: String = or(actor("thread"), "-").chars().take(8).collect();
        format!("{:<6} {thread:<8}", or(actor("agent"), "?"))
    };
    let op = if event.operation == "set" && !event.changed_fields.is_empty() {
        format!("set {}", event.changed_fields.join(","))
    } else {
        event.operation.clone()
    };
    let mut line = Line::new()
        .with(Tone::Dim, at.chars().take(19).collect::<String>())
        .with(Tone::Plain, format!("  {machine:<10} "))
        .with(Tone::Who, format!("{who:<15}"))
        .with(Tone::Plain, format!("  {op:<12} "))
        .with(Tone::Id, str_of(&issue));
    if let Some(title) = title {
        line.push(Tone::Plain, format!("  {title}"));
    }
    if branch.truthy() {
        line.push(Tone::Dim, format!("  ({})", str_of(&branch)));
    }
    (row, line)
}

/// Every machine that wrote to the tracker, with its local display name, how many events it wrote
/// and when it last wrote. With `name` (`ID=ALIAS`), that name is recorded first.
///
/// Each machine's journal length and latest event are the store's projection's: the journal is
/// read only when the projection is not the journal's, to make it (roll-lqjj.31).
pub(crate) fn machines(ctx: &Context<'_>, name: Option<&OsStr>) -> Result<Outcome, Failure> {
    if let Some(name) = name.filter(|n| !n.is_empty()) {
        record_alias(ctx, name)?;
    }
    let projection = projection(ctx.ws)?;
    let aliases = machine_aliases(&ctx.ws.settings);
    let mut rows = Vec::new();
    let mut lines = Vec::new();
    for (replica, events, last) in projection.journals() {
        let last = event_time(last.map_or(0, |id| id.millis()));
        let alias = aliases.get(replica);
        let mut row = Object::new();
        row.insert("machine".into(), Value::from(replica));
        row.insert("alias".into(), Value::from(alias.cloned()));
        row.insert("events".into(), Value::from(i64::from(events)));
        row.insert("last_event".into(), Value::from(last.as_str()));
        let shown = alias
            .filter(|a| !a.is_empty())
            .map_or("(unnamed)", String::as_str);
        lines.push(format!(
            "{replica}  {shown:<14} {events:>6} events  last {}",
            last.chars().take(19).collect::<String>()
        ));
        rows.push(Value::from(row));
    }
    if lines.is_empty() {
        lines.push("No machines yet".to_owned());
    }
    Ok(Outcome::new(Value::Array(rows), lines))
}

/// Records `ID=ALIAS` in `aliases.json`, retaining existing names. An unreadable file is never
/// replaced, and a name that is not UTF-8 is refused, as every text bead writes is.
fn record_alias(ctx: &Context<'_>, name: &OsStr) -> Result<(), Failure> {
    let Some(name) = name.to_str() else {
        return Err(Failure::new(Code::Validation, "text is not valid UTF-8"));
    };
    let (ident, alias) = name.split_once('=').unwrap_or((name, ""));
    let path = ctx.ws.settings.home()?.join("aliases.json");
    let shown = printed_text(path.as_os_str().as_bytes());
    let cannot = |problem: &str| {
        Failure::new(
            Code::Io,
            format!("{shown} cannot be read ({problem}); repair it before adding a name"),
        )
    };
    let mut aliases = match read_json_text(&path) {
        Ok(Value::Object(aliases)) => *aliases,
        Ok(_) => {
            return Err(Failure::new(
                Code::Io,
                format!("{shown} does not hold a JSON object; repair it before adding a name"),
            ));
        }
        Err(Unreadable::Missing) => Object::new(),
        Err(Unreadable::Other(problem)) => return Err(cannot(&problem)),
    };
    aliases.insert(ident.to_owned(), Value::from(alias));
    let written = json::write(&Value::from(aliases), Format::INDENTED)
        .map_err(|e| Failure::new(Code::Validation, e.to_string()))?;
    write_atomic(
        &path,
        written.as_bytes(),
        FileMode::Private,
        &ctx.ws.settings.failpoints,
    )?;
    Ok(())
}
