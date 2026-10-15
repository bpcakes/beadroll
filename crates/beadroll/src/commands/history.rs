//! `bead log` and `bead machines`: who did what, and the machines that did it.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt as _;

use beadroll_core::code::Code;
use beadroll_core::json::{self, Format, Int, Object, Surrogates, Value};
use beadroll_core::op::Op;
use beadroll_core::order::causal_order;
use beadroll_core::text::{printed_text, str_of, surrogateescaped};
use beadroll_core::time::event_time;
use beadroll_store::{
    Unreadable, load_journal, machine_aliases, projection, read_json_keeping_surrogates,
    view_outline, write_atomic,
};

use super::{Context, Failure, Line, Outcome, Tone, resolve_arg};

/// The tracker's events in causal order, excluding `init`, or one issue's events: time, machine,
/// who (the actor's name when it was given one, as its claims are labelled, else its agent and
/// thread), operation (with the fields a `set` changed) and issue. A positive `limit` keeps the
/// last that many; a negative limit leaves that many off the start.
///
/// The events and their order are the journal's, which it reads whole; the keys it names issues
/// by are the store's state's, read after it so that every issue an event names is there
/// (roll-lqjj.31). It replays nothing of its own: the journal is replayed only when the
/// projection is not the journal's, to make it, as for every command.
pub(crate) fn log(
    ctx: &Context<'_>,
    key: Option<&OsStr>,
    limit: Option<&Int>,
) -> Result<Outcome, Failure> {
    let mut journal = load_journal(ctx.ws)?;
    journal.settle();
    let st = view_outline(ctx.ws)?;
    let (order, _) = causal_order(&journal);
    let aliases = machine_aliases(&ctx.ws.settings);
    let wanted = match key.filter(|k| !k.is_empty()) {
        Some(key) => Some(resolve_arg(&st, key)?.uid().to_string()),
        None => None,
    };
    let mut rows: Vec<_> = order
        .iter()
        .filter_map(|id| journal.event(*id))
        .filter(|e| e.op_name() != "init")
        .filter(|e| wanted.is_none() || e.issue() == wanted.as_deref())
        .collect();
    let limit = limit.map_or(0, Int::saturating_i128);
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
        let actor = |key: &str| {
            event
                .actor()
                .filter(|a| a.truthy())
                .and_then(|a| a.get(key))
                .cloned()
                .unwrap_or(Value::Null)
        };
        let at = event_time(event.id().millis());
        let replica = event.replica();
        let machine = aliases.get(replica).map_or(replica, String::as_str);
        let issue = match event.issue() {
            Some(uid) => Value::from(st.key_of(uid)),
            None => Value::Null,
        };
        let branch = event
            .place()
            .filter(|w| w.truthy())
            .and_then(|w| w.get("branch"))
            .cloned()
            .unwrap_or(Value::Null);
        let mut row = Object::new();
        row.insert("at".into(), Value::from(at.as_str()));
        row.insert("machine".into(), Value::from(machine));
        row.insert("agent".into(), actor("agent"));
        row.insert("thread".into(), actor("thread"));
        row.insert("name".into(), actor("name"));
        row.insert("op".into(), Value::from(event.op_name()));
        row.insert("issue".into(), issue.clone());
        row.insert("event".into(), Value::from(event.id().to_string()));
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
        let op = match event.op() {
            Op::Set(fields) if !fields.is_empty() => {
                let names: Vec<&str> = fields.iter().map(|(field, _)| field.name()).collect();
                format!("set {}", names.join(","))
            }
            op => op.name().to_owned(),
        };
        let mut line = Line::new()
            .with(Tone::Dim, at.chars().take(19).collect::<String>())
            .with(Tone::Plain, format!("  {machine:<10} "))
            .with(Tone::Who, format!("{who:<15}"))
            .with(Tone::Plain, format!("  {op:<12} "))
            .with(Tone::Id, str_of(&issue));
        if branch.truthy() {
            line.push(Tone::Dim, format!("  ({})", str_of(&branch)));
        }
        lines.push(line);
        result.push(Value::from(row));
    }
    if lines.is_empty() {
        lines.push("No events".into());
    }
    Ok(Outcome::new(Value::Array(result), lines))
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
/// replaced. Invalid UTF-8 bytes in the name become lone surrogate escapes, and existing lone
/// surrogates survive the JSON rewrite.
fn record_alias(ctx: &Context<'_>, name: &OsStr) -> Result<(), Failure> {
    let bytes = name.as_bytes();
    let (ident, alias) = match bytes.iter().position(|b| *b == b'=') {
        Some(at) => (&bytes[..at], &bytes[at + 1..]),
        None => (bytes, &b""[..]),
    };
    let units: Vec<u16> = surrogateescaped(bytes)
        .into_iter()
        .filter_map(|unit| unit.err().map(|byte| 0xDC00 | u16::from(byte)))
        .collect();
    let avoid = String::from_utf8_lossy(bytes).into_owned();
    let path = ctx.ws.settings.home()?.join("aliases.json");
    let shown = printed_text(path.as_os_str().as_bytes());
    let cannot = |problem: &str| {
        Failure::new(
            Code::Io,
            format!("{shown} cannot be read ({problem}); repair it before adding a name"),
        )
    };
    let (aliases, surrogates) = match read_json_keeping_surrogates(&path, &units, &avoid) {
        Ok((Value::Object(aliases), surrogates)) => (*aliases, surrogates),
        Ok(_) => {
            return Err(Failure::new(
                Code::Io,
                format!("{shown} does not hold a JSON object; repair it before adding a name"),
            ));
        }
        Err(Unreadable::Missing) => {
            let surrogates = Surrogates::stand_in("", &units, &avoid)
                .map(|(_, surrogates)| surrogates)
                .ok_or_else(|| cannot("too many lone surrogates to keep apart"))?;
            (Object::new(), surrogates)
        }
        Err(Unreadable::Other(problem)) => return Err(cannot(&problem)),
    };
    let text = |part: &[u8]| -> String {
        surrogateescaped(part)
            .into_iter()
            .map(|unit| match unit {
                Ok(c) => c,
                Err(byte) => surrogates
                    .char_for(0xDC00 | u16::from(byte))
                    .unwrap_or('\u{FFFD}'),
            })
            .collect()
    };
    let mut aliases = aliases;
    aliases.insert(text(ident), Value::from(text(alias)));
    let written = json::write(&Value::from(aliases), Format::INDENTED)
        .map_err(|e| Failure::new(Code::Validation, e.to_string()))?;
    write_atomic(
        &path,
        surrogates.restore(&written).as_bytes(),
        true,
        &ctx.ws.settings.failpoints,
    )?;
    Ok(())
}
