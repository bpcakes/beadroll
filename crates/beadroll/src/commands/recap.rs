//! Activity across local stores, with a bookmark the CLI saves only after delivering the answer.

use std::collections::BTreeMap;
use std::sync::Arc;

use beadroll_core::code::Code;
use beadroll_core::journal::Event;
use beadroll_core::json::{self, Format, Object, Value};
use beadroll_core::op::Op;
use beadroll_core::order::causal_order;
use beadroll_core::replay::replay;
use beadroll_core::state::State;
use beadroll_core::text::str_of;
use beadroll_core::time::event_time;
use beadroll_store::{
    RecapCursor, Settings, Workspace, load_journal, machine_aliases, recap_workspaces,
};

use super::{Failure, Line, Outcome, Tone};

/// Reads every store before returning an answer. Failed reads never advance any bookmark.
/// `all` includes events already delivered; `advance` bookmarks this answer once the CLI saves it.
pub(crate) fn recap(
    settings: Arc<Settings>,
    all: bool,
    advance: bool,
) -> Result<(Outcome, RecapCursor), Failure> {
    let mut cursor = RecapCursor::open(Arc::clone(&settings), advance)?;
    let aliases = machine_aliases(&settings);
    let mut workspaces = Vec::new();
    let mut lines = Vec::new();
    for ws in recap_workspaces(&settings)? {
        let (workspace, shown) = activity(&ws, &aliases, &mut cursor, all)?;
        workspaces.push(workspace);
        lines.extend(shown);
    }
    if workspaces.is_empty() {
        lines.push("No local trackers".into());
    }
    let mut result = Object::new();
    result.insert("workspaces".into(), Value::Array(workspaces));
    Ok((Outcome::new(result, lines), cursor))
}

/// Reads a journal once. Keeping just the selected events lets replay release the rest while
/// deriving current issue keys and titles, including renamed keys and deleted issues.
fn activity(
    ws: &Workspace,
    aliases: &BTreeMap<String, String>,
    cursor: &mut RecapCursor,
    all: bool,
) -> Result<(Value, Vec<Line>), Failure> {
    let meta = ws.meta(true)?;
    let journal = load_journal(ws)?;
    let (order, _) = causal_order(&journal);
    let events: Vec<Event> = order
        .into_iter()
        .filter(|id| all || !cursor.contains(&ws.id, *id))
        .filter_map(|id| journal.event(id))
        .filter(|event| !matches!(event.op(), Op::Init { .. }))
        .cloned()
        .collect();
    let state = replay(journal).map_err(|e| Failure::new(e.code(), e.to_string()))?;
    let name = meta
        .get("name")
        .filter(|v| v.truthy())
        .or_else(|| state.name())
        .map_or_else(|| ws.id.clone(), str_of);
    let synced = meta.get("last_sync").cloned().unwrap_or(Value::Null);
    let mut lines = vec![Line::toned(Tone::Heading, format!("{name} [{}]", ws.id))];
    lines.push(Line::from(format!(
        "  Local activity; last sync: {}",
        if synced.truthy() {
            str_of(&synced)
        } else {
            "never".into()
        }
    )));
    for problem in state.integrity() {
        lines.push(Line::toned(
            Tone::Warning,
            format!(
                "  Integrity: {}{}",
                problem
                    .origin
                    .as_ref()
                    .map(|p| format!("{p}: "))
                    .unwrap_or_default(),
                problem.problem
            ),
        ));
    }
    if !state.integrity().is_empty() {
        lines.push(Line::toned(
            Tone::Warning,
            "  Cursor kept for this workspace; some records could not be applied.",
        ));
    }
    if !state.pending().is_empty() {
        lines.push(Line::toned(
            Tone::Warning,
            format!(
                "  {} event(s) waiting for causes; left for a later recap.",
                state.pending().len()
            ),
        ));
    }
    let mut rows = Vec::new();
    for event in &events {
        let row = event_row(event, &state, aliases)?;
        lines.extend(event_lines(&row));
        rows.push(Value::from(row));
    }
    if events.is_empty() {
        lines.push("  No new activity".into());
    }
    // A later repair or upgrade may make these records usable. Do not acknowledge a damaged
    // store's answer; healthy stores can still advance. Missing causes are never in `events`.
    if state.integrity().is_empty() {
        cursor.record(&ws.id, events.iter().map(Event::id));
    }
    let mut result = Object::new();
    result.insert("workspace".into(), Value::from(ws.id.as_str()));
    result.insert("name".into(), Value::from(name));
    result.insert("last_sync".into(), synced);
    result.insert("events".into(), Value::Array(rows));
    result.insert("pending".into(), Value::from(state.pending().len() as i64));
    result.insert(
        "integrity".into(),
        Value::Array(
            state
                .integrity()
                .iter()
                .map(|problem| Value::from(problem.to_value()))
                .collect(),
        ),
    );
    Ok((Value::from(result), lines))
}

fn event_row(
    event: &Event,
    state: &State,
    aliases: &BTreeMap<String, String>,
) -> Result<Object, Failure> {
    let mut row = Object::new();
    row.insert("event".into(), Value::from(event.id().to_string()));
    row.insert("at".into(), Value::from(event_time(event.id().millis())));
    row.insert("op".into(), Value::from(event.op_name()));
    row.insert(
        "issue".into(),
        Value::from(event.issue().map(|uid| state.key_of(uid).to_owned())),
    );
    row.insert(
        "title".into(),
        Value::from(
            event
                .issue()
                .and_then(|uid| state.issue_named(uid))
                .map(|issue| issue.title().to_owned()),
        ),
    );
    row.insert("summary".into(), Value::from(summary(event.op(), state)?));
    row.insert("machine".into(), Value::from(event.replica()));
    row.insert(
        "alias".into(),
        Value::from(aliases.get(event.replica()).cloned()),
    );
    for key in ["agent", "thread", "name"] {
        row.insert(
            key.into(),
            event
                .actor()
                .and_then(|actor| actor.get(key))
                .filter(|value| matches!(value, Value::Str(_)))
                .cloned()
                .unwrap_or(Value::Null),
        );
    }
    for key in ["branch", "commit"] {
        row.insert(
            key.into(),
            event
                .place()
                .and_then(|place| place.get(key))
                .filter(|value| matches!(value, Value::Str(_)))
                .cloned()
                .unwrap_or(Value::Null),
        );
    }
    Ok(row)
}

/// A readable description of the recorded operation, not a claim about the issue's current state.
fn summary(op: &Op, state: &State) -> Result<String, Failure> {
    let reason = |name: &str, why: Option<&str>| match why {
        Some(why) if !why.is_empty() => format!("{name}: {why}"),
        _ => name.to_owned(),
    };
    Ok(match op {
        Op::Create(create) if create.imported => "create (imported)".into(),
        Op::Set(fields) => {
            let fields = fields
                .iter()
                .map(|(field, value)| {
                    json::write(value, Format::SPACED)
                        .map(|value| format!("{}={value}", field.name()))
                        .map_err(|e| Failure::new(Code::Validation, e.to_string()))
                })
                .collect::<Result<Vec<_>, _>>()?;
            format!("set {}", fields.join(", "))
        }
        Op::Comment { text, .. } => format!("comment: {text}"),
        Op::Claim(claim) => {
            let name = if claim.mode.as_deref() == Some("offline") {
                "claim (provisional)"
            } else if claim.takeover.is_some() {
                "takeover"
            } else {
                "claim"
            };
            reason(name, claim.reason.as_deref())
        }
        Op::Release(ending) | Op::Close(ending) | Op::Delete(ending) => {
            reason(op.name(), ending.reason.as_deref())
        }
        Op::DepAdd { to, kind, .. } => format!(
            "dep_add {} {}",
            kind.as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or("blocks"),
            state.key_of(to)
        ),
        Op::DepRemove { to, key, kind } => format!(
            "dep_remove {} {}",
            kind.as_deref().unwrap_or("all types"),
            state.key_of(to.as_deref().or(key.as_deref()).unwrap_or("?"))
        ),
        Op::LabelAdd(labels) | Op::LabelRemove(labels) | Op::LabelSet(labels) => {
            format!("{} {}", op.name(), labels.join(", "))
        }
        _ => op.name().to_owned(),
    })
}

fn event_lines(row: &Object) -> Vec<Line> {
    let text = |key: &str| {
        row.get(key)
            .filter(|v| v.truthy())
            .map_or_else(|| "-".to_owned(), str_of)
    };
    let mut lines = vec![
        Line::new()
            .with(Tone::Dim, format!("  {}  ", text("at")))
            .with(Tone::Id, text("issue"))
            .with(Tone::Plain, format!("  {}", text("title"))),
    ];
    lines.push(Line::from(format!("    {}", text("summary"))));
    let alias = row
        .get("alias")
        .filter(|v| v.truthy())
        .map(|alias| format!(" ({})", str_of(alias)))
        .unwrap_or_default();
    lines.push(Line::new().with(
        Tone::Who,
        format!(
            "    machine={}{} agent={} thread={} name={}",
            text("machine"),
            alias,
            text("agent"),
            text("thread"),
            text("name")
        ),
    ));
    lines.push(Line::from(format!(
        "    branch={} commit={}",
        text("branch"),
        text("commit")
    )));
    lines
}
