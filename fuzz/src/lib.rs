//! What bead's fuzz targets share (roll-k2wy.14.4): every JSON dialect, the comparison that tells
//! `0.0` from `-0.0` and one key order from another, events built as `Tx::emit` writes them, and
//! the answers a state gives through the public API, to compare two states by. A target asserts:
//! an assertion that fails is a crash, and libFuzzer keeps its input.

use beadroll_core::event::Field;
use beadroll_core::id::EventId;
use beadroll_core::json::{self, Format, Object, Style, Value};
use beadroll_core::query::{Edges, ReadyScope};
use beadroll_core::state::State;

/// Every format: three layouts, raw or ASCII-escaped, in insertion or key order. The six bead names
/// (`Format::COMPACT` and the rest) are among them.
pub const FORMATS: [Format; 12] = {
    let styles = [Style::Compact, Style::Spaced, Style::Indented];
    let mut all = [Format::COMPACT; 12];
    let mut at = 0;
    while at < 12 {
        all[at] = Format {
            style: styles[at / 4],
            ascii: at % 4 >= 2,
            sorted: at % 2 == 1,
        };
        at += 1;
    }
    all
};

/// Whether `a` and `b` are the same value member for member, in the same order, every float the
/// same bits.
pub fn identical(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Float(a), Value::Float(b)) => a.to_bits() == b.to_bits(),
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| identical(a, b))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && (a.iter().zip(b.iter()))
                    .all(|((ka, va), (kb, vb))| ka == kb && identical(va, vb))
        }
        (a, b) => a == b,
    }
}

/// Whether every value of `a` is identical to the value of `b` at its place.
pub fn all_identical(a: &[Value], b: &[Value]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(a, b)| identical(a, b))
}

/// `value` with every object's members in key order, as a dialect that sorts keys writes it.
pub fn sorted(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
        Value::Object(members) => {
            let mut members: Vec<(&String, &Value)> = members.iter().collect();
            members.sort_by(|a, b| a.0.cmp(b.0));
            Value::from(
                (members.into_iter())
                    .map(|(key, value)| (key.clone(), sorted(value)))
                    .collect::<Object>(),
            )
        }
        other => other.clone(),
    }
}

/// The id of event number `n`: a UUIDv7 of one millisecond in 2026, ids in the order of `n`.
pub fn id(n: u64) -> String {
    let value = (1_790_000_000_000u128 << 80) | (7 << 76) | (2 << 62) | u128::from(n);
    EventId::from_u128(value).to_string()
}

/// Event `id` doing `op` on `issue` (itself, when `None`) with `data`, written by machine
/// `0000aa01` on top of `deps`, as `Tx::emit` writes it.
pub fn event(id: &str, op: &str, issue: Option<&str>, deps: &[String], data: Object) -> Value {
    let mut actor = Object::new();
    actor.insert("agent".into(), Value::from("fuzz"));
    actor.insert("thread".into(), Value::from("t"));
    let mut event = Object::new();
    event.insert("v".into(), Value::from(1));
    event.insert("id".into(), Value::from(id));
    event.insert("replica".into(), Value::from("0000aa01"));
    let deps = deps.iter().map(|dep| Value::from(dep.as_str())).collect();
    event.insert("deps".into(), Value::Array(deps));
    event.insert("actor".into(), Value::from(actor));
    event.insert("op".into(), Value::from(op));
    event.insert("issue".into(), Value::from(issue.unwrap_or(id)));
    event.insert("data".into(), Value::from(data));
    Value::from(event)
}

fn written(value: &Value) -> String {
    json::write(value, Format::CANONICAL).unwrap_or_else(|error| error.to_string())
}

/// What `state` answers through what a command reads from a state alone, each answer with what
/// was asked: every issue's fields, views, conflicts and blockers, readiness, cycles, export, and
/// what replay reported.
pub fn answers(state: &State) -> Vec<(String, String)> {
    let mut said = Vec::new();
    let mut say = |asked: String, answer: String| said.push((asked, answer));
    let shown = |object: Object| written(&Value::from(object));
    say("keys".into(), format!("{:?}", state.keys()));
    let blockers = state.blocker_lists();
    for issue in state.issues() {
        let key = issue.key();
        for field in Field::ALL {
            say(
                format!("{key} {field:?}"),
                format!("{:?}", issue.field(field)),
            );
        }
        say(format!("{key} brief"), shown(state.brief(issue)));
        say(format!("{key} detail"), shown(state.detail(issue)));
        say(format!("{key} line"), state.line(issue));
        say(
            format!("{key} conflicts"),
            format!("{:?}", state.conflicts(issue)),
        );
        let held: Vec<&str> = blockers.of(issue).iter().map(|i| i.key()).collect();
        say(format!("{key} blockers"), format!("{held:?}"));
    }
    let ready: Vec<&str> = state
        .ready(ReadyScope::All)
        .iter()
        .map(|i| i.key())
        .collect();
    say("ready".into(), format!("{ready:?}"));
    for edges in [Edges::WaitsFor, Edges::Blocking] {
        say(
            format!("cycles {edges:?}"),
            format!("{:?}", state.cycles(edges)),
        );
    }
    let export: Vec<String> = state.export_records().into_iter().map(shown).collect();
    say("export".into(), export.join("\n"));
    say("renames".into(), format!("{:?}", state.renames()));
    let mut integrity: Vec<String> = state.integrity().iter().map(|r| format!("{r:?}")).collect();
    integrity.sort();
    say("integrity".into(), integrity.join("\n"));
    say("chain breaks".into(), format!("{:?}", state.chain_breaks()));
    said
}

/// The first answer `b` gives otherwise than `a`, with what was asked, for a crash's message.
pub fn first_difference(a: &[(String, String)], b: &[(String, String)]) -> Option<String> {
    match a.iter().zip(b).find(|(a, b)| a != b) {
        Some(((asked, one), (_, other))) => Some(format!("{asked}:\n  {one}\n  {other}")),
        None if a.len() != b.len() => Some(format!("{} answers, then {}", a.len(), b.len())),
        None => None,
    }
}
