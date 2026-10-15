//! `bead claims`: who holds what, and when each owner was last active.

use beadroll_core::event::Field;
use beadroll_core::id::EventId;
use beadroll_core::json::{Int, Object, Value};
use beadroll_core::state::Issue;
use beadroll_core::text::str_of;
use beadroll_core::time::event_time;
use beadroll_store::{machine_aliases, view_outline};

use super::{Context, Failure, Line, Outcome, Tone, need_tracker};

/// Every claim, by issue key: confirmed owners with machine, thread, generation, branch and time
/// since their thread last wrote; offline claims; and this machine's attempts awaiting the
/// remote. `now` is seconds since the epoch.
pub(crate) fn claims(ctx: &Context<'_>, now: f64) -> Result<Outcome, Failure> {
    let st = view_outline(ctx.ws)?;
    need_tracker(&st)?;
    let aliases = machine_aliases(&ctx.ws.settings);
    let mut issues: Vec<&Issue> = st.issues().collect();
    issues.sort_by(|a, b| a.key().cmp(b.key()));
    let mut rows: Vec<Object> = Vec::new();
    for issue in issues {
        let title = issue.get(Field::Title).cloned().unwrap_or(Value::Null);
        let row = |owner: Value, since: Value| {
            let mut row = Object::new();
            row.insert("id".into(), Value::from(issue.key()));
            row.insert("title".into(), title.clone());
            row.insert("owner".into(), owner);
            row.insert("since".into(), since);
            row
        };
        if let Some(held) = issue.claim() {
            let thread = (
                held.machine.to_string(),
                held.thread.as_deref().map(str::to_owned),
            );
            let last: EventId = st.last_by_thread().get(&thread).copied().unwrap_or(held.id);
            let millis = last.millis();
            let machine = aliases
                .get(&*held.machine)
                .map_or(&*held.machine, String::as_str);
            let branch = held
                .where_
                .as_ref()
                .filter(|place| place.truthy())
                .and_then(|place| place.get("branch"))
                .cloned()
                .unwrap_or(Value::Null);
            // Elapsed minutes, truncating the floating-point result towards zero.
            let idle = ((now - millis as f64 / 1000.0) / 60.0).trunc();
            let mut entry = Object::new();
            entry.insert("id".into(), Value::from(issue.key()));
            entry.insert("title".into(), title.clone());
            entry.insert("owner".into(), Value::from(&*held.owner));
            entry.insert("machine".into(), Value::from(machine));
            entry.insert("agent".into(), Value::from(held.agent.as_deref()));
            entry.insert("thread".into(), Value::from(held.thread.as_deref()));
            entry.insert("generation".into(), Value::Int(held.generation.clone()));
            entry.insert("since".into(), Value::from(held.since()));
            entry.insert("branch".into(), branch);
            entry.insert("state".into(), Value::from("confirmed"));
            entry.insert("last_activity".into(), Value::from(event_time(millis)));
            let idle = Int::from_f64(idle).map_or(Value::Null, Value::Int);
            entry.insert("idle_minutes".into(), idle);
            rows.push(entry);
        }
        for attempt in issue.provisional_claims() {
            let mut entry = row(Value::from(&*attempt.owner), Value::from(attempt.since()));
            entry.insert("state".into(), Value::from("provisional"));
            rows.push(entry);
        }
        for pending in issue.unconfirmed() {
            let part = |key: &str| pending.get(key).cloned().unwrap_or(Value::Null);
            let mut entry = row(part("by"), part("at"));
            let op = pending.get("op").map(str_of).unwrap_or_default();
            entry.insert("state".into(), Value::from(format!("unconfirmed {op}")));
            rows.push(entry);
        }
    }
    let synced = ctx.ws.meta(false)?.get("last_sync").cloned();
    let mut lines: Vec<Line> = rows.iter().map(line).collect();
    if lines.is_empty() {
        lines.push("Nothing is claimed".into());
    }
    let shown = synced
        .as_ref()
        .filter(|v| v.truthy())
        .map_or_else(|| "never".to_owned(), str_of);
    lines.push(Line::toned(
        Tone::Dim,
        format!("(as of this machine's last sync: {shown})"),
    ));
    let mut result = Object::new();
    result.insert(
        "claims".into(),
        Value::Array(rows.into_iter().map(Value::from).collect()),
    );
    result.insert("last_sync".into(), synced.unwrap_or(Value::Null));
    Ok(Outcome::new(result, lines).from_state(st))
}

/// A claim's line: key, state, owner and since; for a confirmed claim its generation and last
/// activity; its branch when it has one.
fn line(row: &Object) -> Line {
    let field = |key: &str| row.get(key).map(str_of).unwrap_or_default();
    let head = |key: &str| field(key).chars().take(16).collect::<String>();
    let state = field("state");
    let tone = if state == "confirmed" {
        Tone::Done
    } else {
        Tone::Warning
    };
    let mut out = Line::new()
        .with(Tone::Id, field("id"))
        .with(Tone::Plain, "  ")
        .with(tone, format!("{state:<11}"))
        .with(Tone::Plain, " ")
        .with(Tone::Who, field("owner"))
        .with(Tone::Plain, format!("  since {}", head("since")));
    if row.contains_key("generation") {
        out.push(
            Tone::Plain,
            format!(
                "  gen {}  last activity {}",
                field("generation"),
                head("last_activity")
            ),
        );
    }
    if row.get("branch").is_some_and(Value::truthy) {
        out.push(Tone::Dim, format!("  on {}", field("branch")));
    }
    out
}
