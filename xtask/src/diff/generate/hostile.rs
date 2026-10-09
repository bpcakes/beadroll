//! Journals as bytes, with what a damaged, hostile or merely odd store holds: envelopes and payloads
//! that fail validation, ids with several contents, copies in the wrong journal, torn tails, text
//! that is not UTF-8, numbers JSON has no room for, and nesting past the ceiling.

use beadroll_core::json::{Format, Object, Value, write};

use super::model::{TEXTS, World};
use crate::diff::report::Op;

/// Where a hostile token goes in an encoded line.
const MARK: &str = "\u{1}MARK\u{1}";

fn text(value: &str) -> Value {
    Value::from(value)
}

/// One event as `encode_events` writes it: compact, UTF-8.
fn line(event: &Object) -> Vec<u8> {
    write(&Value::Object(event.clone()), Format::COMPACT)
        .unwrap_or_default()
        .into_bytes()
}

/// Bytes JSON has no value for, or values at the edge of what bead reads.
fn token(world: &mut World<'_>) -> Vec<u8> {
    let deep = |depth: usize| format!("{}{}", "[".repeat(depth), "]".repeat(depth)).into_bytes();
    let digits = |n: usize, sign: &str| format!("{sign}{}", "7".repeat(n)).into_bytes();
    match world.tape.below(24) {
        0 => b"NaN".to_vec(),
        1 => b"Infinity".to_vec(),
        2 => b"-Infinity".to_vec(),
        3 => b"1e400".to_vec(),
        4 => deep(509),
        5 => deep(510),
        6 => deep(511),
        7 => digits(4300, ""),
        8 => digits(4301, ""),
        9 => digits(4300, "-"),
        10 => digits(4301, "-"),
        11 => b"\"\\ud800\"".to_vec(),
        12 => b"\"a\xed\xa0\x80b\"".to_vec(),
        13 => b"\"\\ud83d\\ude00\"".to_vec(),
        14 => b"\"\\udfff\\ud800\"".to_vec(),
        15 => b"\"\xff\"".to_vec(),
        16 => b"1e16".to_vec(),
        17 => b"1e-05".to_vec(),
        18 => b"-0.0".to_vec(),
        19 => b"5e-324".to_vec(),
        20 => b"{\"\\ud800\":1,\"\\udbff\":2}".to_vec(),
        21 => b"{\"k\":1,\"k\":2}".to_vec(),
        22 => b"-0".to_vec(),
        _ => b"1.5e300".to_vec(),
    }
}

/// The envelope or payload of `event` made invalid, or odd, one way.
fn mutate(world: &mut World<'_>, event: &mut Object) {
    let op = event
        .get("op")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    match world.tape.below(34) {
        0 => drop(event.shift_remove("id")),
        1 => {
            let id = event
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            event.insert(
                "id".into(),
                text(&if world.tape.chance(50) {
                    id.to_uppercase()
                } else {
                    format!("{id}\n")
                }),
            );
        }
        2 => drop(event.insert("id".into(), text("ffffffff-ffff-7fff-bfff-ffffffffffff"))),
        3 => drop(
            event.insert(
                "op".into(),
                [
                    text("bogus"),
                    text("invalid"),
                    text("quarantined"),
                    Value::from(5),
                ][world.tape.below(4)]
                .clone(),
            ),
        ),
        4 => drop(event.shift_remove("replica")),
        5 => drop(event.insert(
            "deps".into(),
            [text("x"), Value::Array(vec![text("nope")]), Value::Null][world.tape.below(3)].clone(),
        )),
        6 => {
            let missing = world.new_id();
            if let Some(Value::Array(deps)) = event.get_mut("deps") {
                deps.push(text(&missing)); // a cause this journal never holds
            }
        }
        7 => drop(event.insert(
            "data".into(),
            [Value::Array(vec![]), text("x"), Value::Null][world.tape.below(3)].clone(),
        )),
        8 => drop(
            event.insert(
                "v".into(),
                [
                    Value::from(2),
                    Value::Bool(true),
                    Value::Bool(false),
                    text("1"),
                    Value::Null,
                    Value::from(0),
                ][world.tape.below(6)]
                .clone(),
            ),
        ),
        9 => drop(event.insert(
            "issue".into(),
            [Value::from(5), Value::Null, Value::Array(vec![])][world.tape.below(3)].clone(),
        )),
        10 => drop(
            event.insert(
                "actor".into(),
                [
                    text("me"),
                    Value::Object([("agent".to_owned(), Value::from(1))].into_iter().collect()),
                ][world.tape.below(2)]
                .clone(),
            ),
        ),
        11 => drop(
            event.insert(
                "where".into(),
                [
                    text("main"),
                    Value::Object(
                        [("branch".to_owned(), Value::from(1))]
                            .into_iter()
                            .collect(),
                    ),
                ][world.tape.below(2)]
                .clone(),
            ),
        ),
        12 => {
            let id = event.get("id").cloned().unwrap_or(Value::Null);
            if let Some(Value::Array(deps)) = event.get_mut("deps") {
                deps.push(id); // waits for itself
            }
        }
        _ => payload(world, &op, event),
    }
}

/// The event's payload, when it is an object.
fn data_of(event: &mut Object) -> Option<&mut Object> {
    match event.get_mut("data") {
        Some(Value::Object(data)) => Some(data),
        _ => None,
    }
}

/// What a hostile payload changes: one field, or one member of the payload.
enum Change {
    Field(&'static str, Value),
    Member(&'static str, Value),
}

fn object(members: &[(&str, Value)]) -> Value {
    Value::Object(
        members
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect(),
    )
}

/// The change choice `choice` makes to a payload of `op`: one validation refuses, or one only
/// replay's looser rules accept.
fn change(op: &str, choice: usize, number: Value) -> Change {
    let pick = |values: Vec<Value>| {
        values
            .get(choice % values.len())
            .cloned()
            .unwrap_or(Value::Null)
    };
    match (op, choice) {
        ("create", 0) => {
            let keys = vec![
                text(""),
                Value::from(1),
                text(&format!("x.{}", "9".repeat(4300))),
                text(&format!("x.{}", "9".repeat(4299))),
            ];
            Change::Member("key", pick(keys))
        }
        ("create" | "set", 1) => Change::Field("created_at", Value::Null),
        ("create" | "set", 2) => Change::Field("title", Value::Null),
        ("create" | "set", 3) => Change::Field(
            "priority",
            pick(vec![Value::from(5), text("2"), Value::from(-1)]),
        ),
        ("create" | "set", 4) => Change::Field(
            "status",
            pick(vec![
                text("Not A Status"),
                text(&"s".repeat(41)),
                text("review"),
            ]),
        ),
        ("create" | "set", 5) => Change::Field("bogus", text("x")),
        ("create" | "set", 6) => Change::Field("estimated_minutes", number),
        ("create", 7) => {
            let comment = object(&[
                ("text", text("t")),
                ("author", Value::Null),
                ("created_at", Value::Null),
                ("uid", Value::from(5)),
            ]);
            Change::Member("comments", Value::Array(vec![comment]))
        }
        ("create", 8) => Change::Member(
            "dangling",
            Value::Array(vec![object(&[("depends_on_id", text(""))])]),
        ),
        ("set", 7) => Change::Member("fields", object(&[("updated_at", text("x"))])),
        ("dep_add", _) => Change::Member(
            "meta",
            pick(vec![
                Value::from(1),
                text("x"),
                Value::Array(vec![Value::from(1)]),
                Value::from(0),
                text(""),
            ]),
        ),
        ("dep_remove", _) => Change::Member(
            "key",
            pick(vec![
                Value::Array(vec![]),
                Value::Object(Object::new()),
                Value::from(1),
            ]),
        ),
        ("label_add" | "label_remove" | "label_set", _) => Change::Member(
            "labels",
            pick(vec![text("bug"), Value::Array(vec![Value::from(1)])]),
        ),
        ("comment", _) => Change::Member(
            if choice.is_multiple_of(2) {
                "text"
            } else {
                "author"
            },
            Value::from(1),
        ),
        ("claim" | "release" | "close" | "reopen" | "delete", _) => {
            let name = ["mode", "takeover", "reason", "claim", "generation"]
                .get(choice % 5)
                .copied()
                .unwrap_or("mode");
            Change::Member(
                name,
                pick(vec![Value::from(1), Value::Null, Value::Float(1.5)]),
            )
        }
        ("init", _) => Change::Member(
            "prefix",
            pick(vec![
                Value::from(5),
                Value::Array(vec![text("a")]),
                Value::Null,
            ]),
        ),
        _ => Change::Member(
            "extra-member",
            text(TEXTS.get(choice).copied().unwrap_or_default()),
        ),
    }
}

/// A payload of `op` that validation refuses, or that only replay's looser rules accept.
fn payload(world: &mut World<'_>, op: &str, event: &mut Object) {
    let choice = world.tape.below(12);
    let number = world.number();
    let Some(data) = data_of(event) else { return };
    match change(op, choice, number) {
        Change::Field(name, value) => {
            let mut fields = data
                .get("fields")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            fields.insert(name.into(), value);
            data.insert("fields".into(), Value::Object(fields));
        }
        Change::Member(name, value) => drop(data.insert(name.into(), value)),
    }
}

/// The lines of one file, damaged now and then.
fn bytes(world: &mut World<'_>, lines: Vec<Vec<u8>>, attempt: bool) -> Vec<u8> {
    let mut out = Vec::new();
    let count = lines.len();
    for (index, mut line) in lines.into_iter().enumerate() {
        if world.hostile(4) {
            match world.tape.below(9) {
                0 => out.extend_from_slice(b"  \t\x0b\x0c\r\n"),
                1 => out.extend_from_slice(b"not json\n"),
                2 => out.extend_from_slice(
                    [b"null\n".as_slice(), b"5\n", b"[]\n", b"\"s\"\n"][world.tape.below(4)],
                ),
                3 => drop(line.splice(0..0, *b"\xef\xbb\xbf")),
                4 => {
                    line = String::from_utf8_lossy(&line)
                        .encode_utf16()
                        .flat_map(u16::to_le_bytes)
                        .collect()
                }
                5 => line.push(b'\r'),
                6 => line.insert(line.len().saturating_sub(1), b' '),
                7 => {
                    let at = line.len() / 2;
                    line.insert(at, 0xff);
                }
                _ => out.extend_from_slice(b"\n"),
            }
        }
        out.extend_from_slice(&line);
        let last = index + 1 == count;
        if !(last && !attempt && world.hostile(10)) {
            out.push(b'\n'); // a final record may lack only its newline
        }
    }
    if !attempt && world.hostile(8) {
        out.extend_from_slice(b"{\"v\":1,\"id\":\"019a"); // torn: another process is mid-append
    }
    out
}

/// One event's line, with a hostile token spliced in now and then.
fn spliced(world: &mut World<'_>, mut event: Object) -> Vec<u8> {
    if !world.hostile(6) {
        return line(&event);
    }
    match event.get_mut("data") {
        Some(Value::Object(data)) => {
            let name = if world.tape.chance(50) { "x" } else { "extra" };
            data.insert(name.into(), text(MARK));
        }
        _ => drop(event.insert("x".into(), text(MARK))),
    }
    let encoded = line(&event);
    let marker = write(&text(MARK), Format::COMPACT)
        .unwrap_or_default()
        .into_bytes();
    let token = token(world);
    let at = encoded
        .windows(marker.len())
        .position(|w| w == marker.as_slice());
    match at {
        Some(at) => [&encoded[..at], &token, &encoded[at + marker.len()..]].concat(),
        None => encoded,
    }
}

/// The world's files as reader operations, damaged and duplicated now and then.
pub(super) fn encode(world: &mut World<'_>) -> Vec<Op> {
    let files = world.files();
    let mut segments = Vec::new();
    let mut attempts = Vec::new();
    let all: Vec<Object> = files
        .iter()
        .flat_map(|(_, journal, _)| journal.iter().cloned())
        .collect();
    for (name, journal, files) in files {
        let events = damaged(world, journal, &all);
        let lines: Vec<Vec<u8>> = events
            .into_iter()
            .map(|event| spliced(world, event))
            .collect();
        segments.extend(split(world, &name, &lines));
        for (number, attempt) in files.into_iter().enumerate() {
            let lines = attempt
                .into_iter()
                .map(|mut event| {
                    if world.hostile(5) {
                        mutate(world, &mut event);
                    }
                    line(&event)
                })
                .collect();
            let data = bytes(world, lines, true);
            attempts.push(Op::Attempt {
                replica: name.clone(),
                origin: format!("{name}/claims/{number}.json"),
                data,
            });
        }
    }
    if world.tape.chance(10) {
        segments.reverse(); // another machine's files read first
    }
    let mut ops = segments;
    if world.hostile(5) {
        ops.push(Op::Settle);
    }
    ops.extend(attempts);
    if world.hostile(8) {
        world.raw_ops(&mut ops);
    }
    if world.tape.chance(25) {
        import(world, &mut ops);
    }
    ops
}

/// A machine's journal, an event mutated now and then, and an id written again now and then: a
/// duplicate, a different content, or someone else's event.
fn damaged(world: &mut World<'_>, journal: Vec<Object>, all: &[Object]) -> Vec<Object> {
    let mut events: Vec<Object> = Vec::new();
    for mut event in journal {
        if world.hostile(6) {
            mutate(world, &mut event);
        }
        events.push(event);
        if world.hostile(3) {
            let mut copy = match all.get(world.tape.below(all.len())).cloned() {
                Some(other) if world.tape.chance(40) => other,
                _ => events.last().cloned().unwrap_or_default(),
            };
            if world.tape.chance(60) {
                copy.insert("data".into(), object(&[("changed", Value::Bool(true))]));
            }
            events.push(copy);
        }
    }
    events
}

/// A machine's lines as one segment, or several.
fn split(world: &mut World<'_>, name: &str, lines: &[Vec<u8>]) -> Vec<Op> {
    let mut segments = Vec::new();
    let mut rest = lines;
    let mut number = 1;
    while !rest.is_empty() {
        let take = if world.tape.chance(30) {
            1 + world.tape.below(rest.len())
        } else {
            rest.len()
        };
        let (chunk, tail) = rest.split_at(take.min(rest.len()));
        let data = bytes(world, chunk.to_vec(), false);
        segments.push(Op::Segment {
            replica: name.to_owned(),
            origin: format!("{name}/{number:06}.jsonl"),
            data,
        });
        rest = tail;
        number += 1;
    }
    segments
}

/// An import among the operations, sometimes twice: the second finds every record known.
fn import(world: &mut World<'_>, ops: &mut Vec<Op>) {
    let known = world.known_keys();
    let text = world.import_text(&known);
    let at = if world.tape.chance(80) {
        ops.len()
    } else {
        world.tape.below(ops.len() + 1)
    };
    let import = Op::Import {
        replica: "0000aa".into(),
        path: "beads.jsonl".into(),
        text,
    };
    if world.tape.chance(30) {
        ops.insert(at, import.clone());
    }
    ops.insert(at, import);
}
