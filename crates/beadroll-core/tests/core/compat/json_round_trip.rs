//! JSON round-trips in every dialect bead writes (roll-k2wy.14.4): a value written by
//! `json::write` in any `Format` (the six bead names, `COMPACT` for journal lines, attempt files
//! and export, `SPACED` for `--json`, `ASCII` for errors, `INDENTED` for the store's small files,
//! `CANONICAL` for quarantine and `SORTED_ASCII` for the manifest and receipts, and the other six
//! combinations of layout, escaping and key order) reads back as the same value, member for member
//! and float for float, and writes again as the same text; every finite float is written in the
//! fewest digits that read back as it; and what bead builds on the writer reads back too: export's
//! records through the
//! importer's reader, the views `--json` prints and the error envelope. The journal's own lines are
//! the store's (`beadroll-store`'s `storage::journal_lines`). The values are drawn by
//! `beadroll_test_support::values`, seeded: a failure names its seed.

use beadroll_core::import::{Step, Target, plan_import, read_records};
use beadroll_core::json::{self, Format, MAX_DEPTH, Object, Style, Value, float_repr};
use beadroll_core::state::State;
use beadroll_test_support::events::{Event, id, state};
use beadroll_test_support::json;
use beadroll_test_support::values::{Rng, identical, sorted};

/// Every format: three layouts, raw or ASCII-escaped, in insertion or key order.
fn dialects() -> Vec<Format> {
    let mut all = Vec::new();
    for style in [Style::Compact, Style::Spaced, Style::Indented] {
        for ascii in [false, true] {
            for sorted in [false, true] {
                all.push(Format {
                    style,
                    ascii,
                    sorted,
                });
            }
        }
    }
    all
}

/// `value` written in `format`, checked: it reads back as `value` (with its keys sorted, for a
/// format that sorts them), writes again as itself, holds nothing but printable ASCII when
/// escaped, and is one line unless indented.
fn round_trip(value: &Value, format: Format) -> Result<String, String> {
    let text = json::write(value, format).map_err(|error| format!("not written: {error}"))?;
    if format.ascii
        && !text
            .bytes()
            .all(|b| b == b'\n' || (b' '..=b'~').contains(&b))
    {
        return Err(format!("not ASCII: {text}"));
    }
    if format.style != Style::Indented && text.contains('\n') {
        return Err(format!("not one line: {text}"));
    }
    let read =
        json::parse(text.as_bytes()).map_err(|error| format!("not read: {error}: {text}"))?;
    let expected = if format.sorted {
        sorted(value)
    } else {
        value.clone()
    };
    if !identical(&read.value, &expected) {
        return Err(format!("read back otherwise: {text}"));
    }
    match json::write(&read.value, format) {
        Ok(again) if again == text => Ok(text),
        again => Err(format!("written again otherwise: {text}\n{again:?}")),
    }
}

#[test]
fn every_dialect_reads_back_what_it_wrote() {
    let formats = dialects();
    for named in [
        Format::COMPACT,
        Format::SPACED,
        Format::ASCII,
        Format::INDENTED,
        Format::CANONICAL,
        Format::SORTED_ASCII,
    ] {
        assert!(formats.contains(&named), "{named:?}");
    }
    for seed in 0..5_000 {
        let mut rng = Rng::new(seed);
        let value = rng.value(4);
        for format in &formats {
            if let Err(problem) = round_trip(&value, *format) {
                panic!("seed {seed}, {format:?}: {problem}");
            }
        }
    }
}

#[test]
fn values_nested_as_deep_as_a_reader_takes_read_back() {
    let mut rng = Rng::new(7);
    let mut deepest = rng.value(0);
    for depth in 1..=MAX_DEPTH {
        deepest = if depth % 2 == 0 {
            Value::Array(vec![deepest])
        } else {
            Value::from(Object::from_iter([(rng.text(3), deepest)]))
        };
    }
    for format in dialects() {
        round_trip(&deepest, format).unwrap();
    }
    // One level deeper is written, as a value read and wrapped may be, but never read.
    let deeper = Value::Array(vec![deepest]);
    let text = json::write(&deeper, Format::COMPACT).unwrap();
    let error = json::parse(text.as_bytes()).unwrap_err().to_string();
    assert!(error.starts_with("recursion limit exceeded"), "{error}");
}

/// The significant digits of a float's text: its mantissa's digits, without the zeros that lead
/// or trail them.
fn significant(text: &str) -> String {
    let mantissa = text.split(['e', 'E']).next().unwrap_or_default();
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    digits.trim_matches('0').to_owned()
}

#[test]
fn every_finite_float_is_written_in_the_fewest_digits_that_read_back_as_it() {
    let mut rng = Rng::new(20_261_016);
    for _ in 0..50_000 {
        let float = rng.float();
        let text = float_repr(float).unwrap();
        let read = json::parse(text.as_bytes()).unwrap().value;
        let Value::Float(back) = read else {
            panic!("{float:e} written as {text}, which reads as {read:?}");
        };
        assert_eq!(
            back.to_bits(),
            float.to_bits(),
            "{float:e} written as {text}"
        );
        // Rust's own shortest form has as many digits; the two may differ in the last of them,
        // where bead breaks a tie toward the even digit as Python does.
        let shortest = significant(&format!("{float:e}"));
        assert_eq!(
            significant(&text).len(),
            shortest.len(),
            "{float:e} written as {text}"
        );
    }
}

const STATUSES: [&str; 7] = [
    "open",
    "in_progress",
    "blocked",
    "deferred",
    "closed",
    "tombstone",
    "triage-2",
];
const KINDS: [&str; 4] = ["blocks", "parent-child", "related", "discovered-from"];

/// A Beads export of `count` records, drawn: every field bead knows with a value of its type,
/// labels, comments, dependencies on each other and on keys nobody has, and members bead does not
/// know, of any value.
fn export_of(rng: &mut Rng, count: usize) -> Vec<Value> {
    (0..count)
        .map(|n| {
            let mut record = Object::new();
            for _ in 0..rng.below(4) {
                record.insert(format!("x-{}", rng.text(6)), rng.value(3));
            }
            record.insert("id".into(), Value::from(format!("rt-{n}")));
            record.insert("title".into(), Value::from(format!("t{}", rng.text(20))));
            record.insert("description".into(), Value::from(rng.text(60)));
            let status = STATUSES[rng.below(STATUSES.len())];
            record.insert("status".into(), Value::from(status));
            record.insert("priority".into(), Value::from(rng.below(5) as i64));
            record.insert("issue_type".into(), Value::from(rng.text(8)));
            record.insert("created_at".into(), Value::from(rng.text(24)));
            let labels: Vec<Value> = (0..rng.below(3))
                .map(|_| Value::from(rng.text(8)))
                .collect();
            record.insert("labels".into(), Value::Array(labels));
            let comments: Vec<Value> = (0..rng.below(3))
                .map(|_| {
                    json!({
                        "id": (Value::Int(rng.int())), "author": (rng.text(8)),
                        "text": (rng.text(40)), "created_at": (rng.text(24))
                    })
                })
                .collect();
            record.insert("comments".into(), Value::Array(comments));
            let targets: Vec<usize> = (0..rng.below(3))
                .map(|_| rng.below(count + 2))
                .filter(|target| *target != n)
                .collect();
            let deps: Vec<Value> = (targets.into_iter())
                .map(|target| {
                    let to = if target < count {
                        format!("rt-{target}")
                    } else {
                        format!("gone-{target}")
                    };
                    let kind = KINDS[rng.below(KINDS.len())];
                    json!({"depends_on_id": to, "type": kind, "metadata": (rng.value(2))})
                })
                .collect();
            record.insert("dependencies".into(), Value::Array(deps));
            Value::from(record)
        })
        .collect()
}

/// The tracker `records` imported into a fresh one make, its events as an import emits them.
fn imported(records: &[Value]) -> State {
    let mut events = vec![
        Event::new(&id(1), "init")
            .data(json!({"prefix": "rt"}))
            .json(),
    ];
    let plan = plan_import(&state(&events), records).unwrap();
    let mut created = Vec::new();
    for step in plan.steps {
        let at = id(events.len() as u64 + 1);
        let last = id(events.len() as u64);
        let event = match step {
            Step::Create(data) => {
                created.push(at.clone());
                Event::new(&at, "create").data(Value::from(data))
            }
            Step::DepAdd { issue, to, data } => {
                let to = match to {
                    Target::Created(n) => created[n].clone(),
                    Target::Issue(uid) => uid.to_string(),
                };
                let mut payload = Object::from_iter([("to".to_owned(), Value::from(to))]);
                payload.extend(data);
                Event::new(&at, "dep_add")
                    .issue(&created[issue])
                    .data(Value::from(payload))
            }
            Step::DepRemove { .. } => panic!("a fresh tracker holds nothing to take back"),
        };
        events.push(event.deps(&[&last]).json());
    }
    state(&events)
}

#[test]
fn exports_views_and_errors_read_back_as_bead_wrote_them() {
    for seed in 0..200 {
        let mut rng = Rng::new(seed);
        let count = 1 + rng.below(12);
        let records = export_of(&mut rng, count);
        let st = imported(&records);
        // `bead export`: a compact line a record, which the importer's reader takes back whole.
        let export = st.export_records();
        let mut text = String::new();
        for record in &export {
            text.push_str(&json::write(&Value::from(record.clone()), Format::COMPACT).unwrap());
            text.push('\n');
        }
        let back = read_records(&text, "export.jsonl").unwrap();
        assert_eq!(back.len(), export.len(), "seed {seed}");
        for (read, record) in back.iter().zip(&export) {
            assert!(
                identical(read, &Value::from(record.clone())),
                "seed {seed}: {text}"
            );
        }
        // what the importer did not know travels through the journal and out again as it came
        for record in &records {
            let exported = (back.iter())
                .find(|exported| exported.get("id") == record.get("id"))
                .unwrap();
            for (name, value) in record.as_object().unwrap() {
                if name.starts_with("x-") {
                    let out = exported.get(name).unwrap_or(&Value::Null);
                    assert!(identical(out, value), "seed {seed}: {name}");
                }
            }
        }
        assert_eq!(plan_import(&st, &back).unwrap().imported, 0, "seed {seed}");
        // `--json`: the views, spaced and raw, and the error envelope, ASCII-escaped
        for issue in st.issues() {
            for view in [st.brief(issue), st.detail(issue)] {
                round_trip(&Value::from(view), Format::SPACED).unwrap();
            }
        }
        let error = json!({"error": {"code": "VALIDATION", "message": (rng.text(80))}});
        round_trip(&error, Format::ASCII).unwrap();
    }
}
