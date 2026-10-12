//! `PortAlignment`, journal lines: what bytes a line may hold and still be an event (U01 to U04).

use beadroll_core::event::event_problem;
use beadroll_core::journal::{Journal, read_segment};
use beadroll_core::json::{Format, MAX_DEPTH, nesting, parse, write};
use beadroll_core::replay::replay;
use beadroll_test_support::{Json, json};

use super::{event, id_of, problems, with};

/// Init event `number`, carrying `data`, as one compact line.
fn line(number: u64, data: Json) -> String {
    write(
        &event(number, "init", "a", &[], None, data),
        Format::COMPACT,
    )
    .unwrap()
}

/// A segment of these lines, each terminated: what it yields and what it reports.
fn read(lines: &[Vec<u8>]) -> (Vec<Json>, Vec<String>) {
    let data: Vec<u8> = lines
        .iter()
        .flat_map(|line| line.iter().copied().chain(*b"\n"))
        .collect();
    let mut problems = Vec::new();
    let events = read_segment(&data, "a/000001.jsonl", Some(&mut problems));
    (
        events,
        problems
            .into_iter()
            .map(|problem| problem.problem)
            .collect(),
    )
}

fn ids(events: &[Json]) -> Vec<&str> {
    events.iter().map(id_of).collect()
}

/// U01: 1e400 read as infinity, which no export can write; like NaN, it is refused.
#[test]
fn u01_numbers_beyond_a_float_are_not_json_numbers() {
    for text in [
        "1e400",
        "-1e400",
        "1E400",
        "[2.5e308]",
        "{\"x\": NaN}",
        "Infinity",
    ] {
        let error = parse(text.as_bytes()).unwrap_err().to_string();
        assert!(error.contains("is not a JSON number"), "{text}: {error}");
    }
    let read_back = parse(b"[1e308, -0.0, 5e-324]").unwrap().value;
    assert_eq!(
        read_back,
        Json::Array(vec![
            Json::Float(1e308),
            Json::Float(-0.0),
            Json::Float(5e-324)
        ])
    );
    let first = line(1, json!({})).replace("\"data\":{}", "\"data\":{\"x\":1e400}");
    let (events, problems) = read(&[first.into_bytes(), line(2, json!({})).into_bytes()]);
    assert_eq!(
        ids(&events),
        [id_of(&event(2, "init", "a", &[], None, json!({})))]
    );
    assert_eq!(problems, ["line is not valid JSON"]);
}

/// U02: `$` let an id or a dependency end in a newline; ids match the UUID shape whole.
#[test]
fn u02_ids_and_deps_with_a_trailing_newline_are_not_uuids() {
    let create = event(1, "init", "a", &[], None, json!({"prefix": "ex"}));
    assert_eq!(event_problem(&create), None);
    let id = format!("{}\n", id_of(&create));
    let renamed = with(&create, "id", json!(id.as_str()));
    assert_eq!(event_problem(&renamed).as_deref(), Some("id is not a UUID"));
    let deps = with(&create, "deps", json!([(id.as_str())]));
    assert_eq!(
        event_problem(&deps).as_deref(),
        Some("deps is not a list of event ids")
    );
    let mut journal = Journal::new();
    journal.add(renamed, "a", "", true);
    let state = replay(journal).unwrap();
    assert!(state.prefix().is_none());
    assert_eq!(problems(&state), ["id is not a UUID"]);
}

/// U03: readers accept at most 512 nesting levels; brackets inside strings do not count.
#[test]
fn u03_nesting_deeper_than_512_levels_is_unreadable() {
    let nested = |depth: usize| format!("{}{}", "[".repeat(depth), "]".repeat(depth));
    assert_eq!(MAX_DEPTH, 512);
    let value = parse(nested(512).as_bytes()).unwrap().value;
    assert_eq!(
        nesting(write(&value, Format::SPACED).unwrap().as_bytes()),
        512
    );
    let error = parse(nested(513).as_bytes()).unwrap_err().to_string();
    assert!(error.contains("nested deeper than 512 levels"), "{error}");
    let brackets = "[{".repeat(600);
    let text = format!("[\"{brackets}\"]");
    assert_eq!(
        parse(text.as_bytes()).unwrap().value,
        json!([(brackets.as_str())])
    );
    // The event's data adds one level.
    let (deep, fine) = (nested(511), nested(510));
    let first = line(1, json!({})).replace("\"data\":{}", &format!("\"data\":{{\"x\":{deep}}}"));
    let second = line(2, json!({})).replace("\"data\":{}", &format!("\"data\":{{\"x\":{fine}}}"));
    let (events, problems) = read(&[first.into_bytes(), second.into_bytes()]);
    assert_eq!(
        ids(&events),
        [id_of(&event(2, "init", "a", &[], None, json!({})))]
    );
    assert_eq!(problems, ["line is not valid JSON"]);
}

/// U04: journal lines must be UTF-8; refuse UTF-16, UTF-32 and a UTF-8 byte-order mark.
#[test]
fn u04_journal_lines_are_utf8_whatever_their_first_bytes() {
    let text = line(1, json!({}));
    let utf16 = |big: bool| -> Vec<u8> {
        text.encode_utf16()
            .flat_map(|unit| {
                if big {
                    unit.to_be_bytes()
                } else {
                    unit.to_le_bytes()
                }
            })
            .collect()
    };
    // UTF-32 with a byte-order mark followed by little-endian code units.
    let utf32: Vec<u8> = [0xff, 0xfe, 0, 0]
        .into_iter()
        .chain(text.chars().flat_map(|c| u32::from(c).to_le_bytes()))
        .collect();
    let latin1: Vec<u8> = text
        .replace("\"test\"", "\"tést\"")
        .chars()
        .map(|c| u8::try_from(u32::from(c)).unwrap())
        .collect();
    let cases = [
        (
            "bom",
            [b"\xef\xbb\xbf".as_slice(), text.as_bytes()].concat(),
        ),
        ("utf-16-le", utf16(false)),
        ("utf-16-be", utf16(true)),
        ("utf-32", utf32),
        ("latin-1", latin1),
    ];
    let second = event(2, "init", "a", &[], None, json!({}));
    for (name, bytes) in cases {
        let (events, problems) = read(&[bytes, line(2, json!({})).into_bytes()]);
        assert_eq!(ids(&events), [id_of(&second)], "{name}");
        assert_eq!(problems, ["line is not valid JSON"], "{name}");
    }
    // Still UTF-8, surrogates and all: a lone surrogate reads as U+FFFD, reported.
    let (events, problems) = read(&[text.replace("\"test\"", "\"t\\ud800\"").into_bytes()]);
    assert_eq!(events[0]["actor"]["agent"], json!("t\u{fffd}"));
    assert_eq!(problems, ["text is not valid UTF-8; it reads as U+FFFD"]);
}
