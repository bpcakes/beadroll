//! Histories every run includes: the boundaries the decided rows of `docs/RUST-PORT.md` §5 draw,
//! and the corners of replay a random tape rarely reaches.

use beadroll_core::json::{Format, Object, Value, write};
use beadroll_core::text::b36;
use sha2::{Digest, Sha256};

use crate::diff::cases::Case;
use crate::diff::report::Op;

fn id(n: u64) -> String {
    format!("019a5555-5555-7000-8000-{n:012x}")
}

fn text(value: &str) -> Value {
    Value::from(value)
}

fn obj(members: &[(&str, Value)]) -> Value {
    Value::Object(
        members
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect(),
    )
}

/// An event as `Tx.emit` writes it.
fn event(n: u64, op: &str, replica: &str, deps: &[u64], issue: Option<u64>, data: Value) -> Value {
    let mut event = Object::new();
    event.insert("v".into(), Value::from(1));
    event.insert("id".into(), text(&id(n)));
    event.insert("replica".into(), text(replica));
    event.insert(
        "deps".into(),
        Value::Array(deps.iter().map(|d| text(&id(*d))).collect()),
    );
    event.insert(
        "actor".into(),
        obj(&[("agent", text("test")), ("thread", text(replica))]),
    );
    event.insert("op".into(), text(op));
    event.insert("issue".into(), text(&id(issue.unwrap_or(n))));
    event.insert("data".into(), data);
    Value::Object(event)
}

fn line(event: &Value) -> Vec<u8> {
    let mut out = write(event, Format::COMPACT)
        .unwrap_or_default()
        .into_bytes();
    out.push(b'\n');
    out
}

fn segment(replica: &str, events: &[Value]) -> Op {
    Op::Segment {
        replica: replica.into(),
        origin: format!("{replica}/000001.jsonl"),
        data: events.iter().flat_map(line).collect(),
    }
}

fn raw(replica: &str, data: Vec<u8>) -> Op {
    Op::Segment {
        replica: replica.into(),
        origin: format!("{replica}/000002.jsonl"),
        data,
    }
}

fn case(name: &str, ops: Vec<Op>) -> Case {
    Case {
        name: format!("scenario: {name}"),
        ops,
    }
}

fn init() -> Value {
    event(
        1,
        "init",
        "a",
        &[],
        None,
        obj(&[("prefix", text("ex")), ("name", text("ex"))]),
    )
}

fn create(n: u64, replica: &str, deps: &[u64], key: &str, fields: Value) -> Value {
    event(
        n,
        "create",
        replica,
        deps,
        None,
        obj(&[("key", text(key)), ("fields", fields)]),
    )
}

/// Every free key a renamed copy could take is taken by imported dependencies: replay fails with
/// `KEY_SPACE`, as Python's does.
fn key_space() -> Case {
    let digest = b36(&Sha256::digest(id(4).as_bytes()));
    let dangling: Vec<Value> = (1..digest.len())
        .map(|size| {
            obj(&[(
                "depends_on_id",
                text(&format!("ex-k{}", digest.get(..size).unwrap_or_default())),
            )])
        })
        .collect();
    let first = event(
        2,
        "create",
        "a",
        &[1],
        None,
        obj(&[("key", text("ex-k")), ("dangling", Value::Array(dangling))]),
    );
    let copy = create(4, "b", &[1], "ex-k", obj(&[]));
    case(
        "every free key taken",
        vec![segment("a", &[init(), first]), segment("b", &[copy])],
    )
}

/// Numbers in keys at the digit limit: 4,299 nines count up to 4,300 digits; 4,300 are refused.
fn key_digits() -> Case {
    let nines = |n: usize| format!("x.{}", "9".repeat(n));
    let ops = vec![
        segment(
            "a",
            &[
                init(),
                create(2, "a", &[1], &nines(4299), obj(&[])),
                create(3, "a", &[2], &nines(4300), obj(&[])),
            ],
        ),
        segment("b", &[create(4, "b", &[1], &nines(4299), obj(&[]))]),
    ];
    case("key numbers at the digit limit", ops)
}

/// Integers at the limit in extras and fields, through replay and export.
fn integers() -> Case {
    let mut data = Vec::new();
    for (n, digits) in [
        (2u64, "7".repeat(4300)),
        (3, format!("-{}", "7".repeat(4300))),
        (4, "7".repeat(4301)),
        (5, format!("-{}", "7".repeat(4301))),
    ] {
        let create = create(
            n,
            "a",
            &[1],
            &format!("ex-{n}"),
            obj(&[("estimated_minutes", text("MARK"))]),
        );
        let text = String::from_utf8(line(&create))
            .unwrap_or_default()
            .replace("\"MARK\"", &digits);
        data.extend_from_slice(text.as_bytes());
    }
    let generation = event(
        6,
        "claim",
        "a",
        &[1],
        Some(2),
        obj(&[("generation", text("MARK"))]),
    );
    let generation = String::from_utf8(line(&generation))
        .unwrap_or_default()
        .replace("\"MARK\"", &"9".repeat(4300));
    data.extend_from_slice(generation.as_bytes());
    let takeover = event(
        7,
        "claim",
        "a",
        &[6],
        Some(2),
        obj(&[("takeover", text(&id(6)))]),
    );
    data.extend(line(&takeover));
    case(
        "integers at the digit limit",
        vec![segment("a", &[init()]), raw("a", data)],
    )
}

/// Rows 27 to 29: an imported comment with a uid or a where, a cleared creation time, and an id
/// with several contents whose first copy names a list.
fn aligned_rows() -> Case {
    let comment = obj(&[
        ("text", text("t")),
        ("author", Value::Null),
        ("created_at", Value::Null),
        ("where", Value::Null),
    ]);
    let imported = event(
        2,
        "create",
        "a",
        &[1],
        None,
        obj(&[
            ("key", text("ex-c")),
            ("comments", Value::Array(vec![comment])),
        ]),
    );
    let cleared = create(3, "a", &[2], "ex-t", obj(&[("created_at", Value::Null)]));
    let mut first = event(5, "create", "a", &[3], None, obj(&[]));
    let mut second = first.clone();
    if let (Value::Object(first), Value::Object(second)) = (&mut first, &mut second) {
        first.insert("issue".into(), Value::Array(vec![Value::from(1)]));
        second.insert("issue".into(), Value::Array(vec![Value::from(1)]));
        second.insert("data".into(), obj(&[("x", Value::from(1))]));
    }
    case(
        "rows 27 to 29",
        vec![
            segment("a", &[init(), imported, cleared, first]),
            segment("b", &[second]),
        ],
    )
}

/// Row 22: a line in UTF-16, one with a byte-order mark, and lone surrogates escaped and raw,
/// including two keys that become one.
fn encodings() -> Case {
    let plain = String::from_utf8(line(&create(
        2,
        "a",
        &[1],
        "ex-u",
        obj(&[("title", text("MARK"))]),
    )))
    .unwrap_or_default();
    let mut data: Vec<u8> = plain.encode_utf16().flat_map(u16::to_le_bytes).collect();
    data.extend_from_slice(b"\xef\xbb\xbf");
    data.extend_from_slice(plain.as_bytes());
    data.extend_from_slice(
        plain
            .replace("\"MARK\"", "\"\\ud800\"")
            .replace("ex-u", "ex-v")
            .replace("002", "003")
            .as_bytes(),
    );
    let raw_surrogate = plain
        .replace("ex-u", "ex-w")
        .replace("002", "004")
        .replace("MARK", "a\u{fffd}b")
        .into_bytes();
    let raw_surrogate = String::from_utf8_lossy(&raw_surrogate)
        .replace('\u{fffd}', "\u{1}")
        .into_bytes();
    data.extend(raw_surrogate.iter().flat_map(|b| {
        if *b == 1 {
            vec![0xed, 0xa0, 0x80]
        } else {
            vec![*b]
        }
    }));
    let keys = plain
        .replace(
            "\"title\":\"MARK\"",
            "\"title\":\"k\",\"\\ud800\":1,\"\\udbff\":2",
        )
        .replace("ex-u", "ex-x")
        .replace("002", "005");
    data.extend_from_slice(keys.as_bytes());
    case("encodings", vec![segment("a", &[init()]), raw("a", data)])
}

/// Row 7: lines nesting 511, 512 and 513 levels deep.
fn nesting() -> Case {
    let mut data = Vec::new();
    for (n, depth) in [(2u64, 509usize), (3, 510), (4, 511)] {
        let deep = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
        let created = String::from_utf8(line(&create(n, "a", &[1], &format!("ex-{n}"), obj(&[]))))
            .unwrap_or_default();
        data.extend_from_slice(
            created
                .replace(
                    "\"fields\":{}",
                    &format!("\"fields\":{{}},\"extra\":{deep}"),
                )
                .as_bytes(),
        );
    }
    case(
        "nesting at the ceiling",
        vec![segment("a", &[init()]), raw("a", data)],
    )
}

/// Ownership: takeovers of a claim that is not one, of a claim not seen, offline, and ends of a
/// superseded claim.
fn ownership() -> Case {
    let issue = create(2, "a", &[1], "ex-o", obj(&[]));
    let claim = event(3, "claim", "a", &[2], Some(2), obj(&[]));
    let blind = event(
        4,
        "claim",
        "b",
        &[2],
        Some(2),
        obj(&[("takeover", text(&id(3)))]),
    );
    let wrong = event(
        5,
        "claim",
        "b",
        &[4],
        Some(2),
        obj(&[("takeover", text(&id(2)))]),
    );
    let takeover = event(
        6,
        "claim",
        "c",
        &[3],
        Some(2),
        obj(&[("takeover", text(&id(3))), ("reason", text("new session"))]),
    );
    let late_release = event(
        7,
        "release",
        "a",
        &[3],
        Some(2),
        obj(&[("claim", text(&id(3)))]),
    );
    let late_close = event(
        8,
        "close",
        "a",
        &[7],
        Some(2),
        obj(&[("claim", text(&id(3)))]),
    );
    let offline = event(
        9,
        "claim",
        "b",
        &[6],
        Some(2),
        obj(&[("mode", text("offline"))]),
    );
    let close = event(
        10,
        "close",
        "c",
        &[6, 9],
        Some(2),
        obj(&[("claim", text(&id(6))), ("reason", text("done"))]),
    );
    let ops = vec![
        segment("a", &[init(), issue, claim, late_release, late_close]),
        segment("b", &[blind, wrong, offline]),
        Op::Attempt {
            replica: "c".into(),
            origin: "c/claims/1.json".into(),
            data: [line(&takeover), line(&close)].concat(),
        },
    ];
    case("ownership", ops)
}

/// More cycles than are reported: seven issues each blocking every other.
fn cycles() -> Case {
    let mut events = vec![init()];
    for n in 2..9u64 {
        events.push(create(
            n,
            "a",
            &[n - 1],
            &format!("ex-{n}"),
            obj(&[("issue_type", text(if n == 2 { "epic" } else { "task" }))]),
        ));
    }
    let mut last = 8;
    let mut next = 100;
    for from in 2..9u64 {
        for to in 2..9u64 {
            if from != to {
                let kind = if to == 2 { "parent-child" } else { "blocks" };
                events.push(event(
                    next,
                    "dep_add",
                    "a",
                    &[last],
                    Some(from),
                    obj(&[("to", text(&id(to))), ("type", text(kind))]),
                ));
                last = next;
                next += 1;
            }
        }
    }
    case("more cycles than are reported", vec![segment("a", &events)])
}

/// An imported comment whose id is the one a comment made here derives.
fn comment_ids() -> Case {
    let comment = event(
        3,
        "comment",
        "a",
        &[2],
        Some(2),
        obj(&[("text", text("ours"))]),
    );
    let derived = beadroll_core::export::comment_id(
        beadroll_core::id::EventId::parse(&id(3))
            .unwrap_or_else(|| beadroll_core::id::EventId::from_u128(0)),
    );
    let legacy = obj(&[
        (
            "legacy_id",
            Value::from(i64::try_from(derived).unwrap_or(0)),
        ),
        ("author", text("bob")),
        ("text", text("theirs")),
        ("created_at", Value::Null),
    ]);
    let imported = event(
        2,
        "create",
        "a",
        &[1],
        None,
        obj(&[
            ("key", text("ex-i")),
            ("comments", Value::Array(vec![legacy])),
            ("imported", Value::Bool(true)),
        ]),
    );
    case(
        "comment ids that collide",
        vec![segment("a", &[init(), imported, comment])],
    )
}

/// A machine identity copied to a second place: both write into one journal.
fn copied_identity() -> Case {
    let a2 = create(2, "a", &[1], "ex-1", obj(&[]));
    let a3 = create(3, "a", &[1], "ex-2", obj(&[])); // does not build on 2
    let a4 = event(
        4,
        "set",
        "a",
        &[3],
        Some(2),
        obj(&[("fields", obj(&[("title", text("t"))]))]),
    );
    case("copied identity", vec![segment("a", &[init(), a2, a3, a4])])
}

pub(super) fn all() -> Vec<Case> {
    vec![
        key_space(),
        key_digits(),
        integers(),
        aligned_rows(),
        encodings(),
        nesting(),
        ownership(),
        cycles(),
        comment_ids(),
        copied_identity(),
    ]
}
