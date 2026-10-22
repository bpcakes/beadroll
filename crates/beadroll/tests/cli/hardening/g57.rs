//! G57: a lone surrogate in a journal is scoped to the line containing it. Attempt-file checks
//! read `claims/*.json` in beadroll-store's `tests/store/hardening.rs`.

use std::time::{SystemTime, UNIX_EPOCH};

use beadroll_core::id::EventId;
use beadroll_core::json::{Format, write};
use beadroll_test_support::{Json, json};

/// What stands for the lone surrogate U+D800 in a title until the line is bytes: a Rust string
/// cannot hold one.
const SURROGATE: &str = "<lone surrogate>";

/// `_g57_event_id`: a UUIDv7 for now, with `n` in its random bits.
fn event_id(n: u128) -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    EventId::from_u128((millis << 80) | (0x7 << 76) | (0b10 << 62) | n).to_string()
}

/// `_g57_create`: event `n`, machine `replica` creating issue `key` titled `title`.
fn create(n: u128, replica: &str, key: &str, title: &str) -> Json {
    let eid = event_id(n);
    json!({
        "v": 1, "id": (eid.as_str()), "replica": replica, "deps": [], "actor": {"agent": "human"},
        "op": "create", "issue": (eid.as_str()), "data": {"key": key, "fields": {"title": title}}
    })
}

/// The event as a JSON line with `SURROGATE` encoded as `\ud800`.
fn escaped_line(event: &Json) -> Vec<u8> {
    let line = write(event, Format::ASCII).unwrap();
    line.replace(SURROGATE, r"\ud800").into_bytes()
}

/// A journal line containing the raw surrogate encoding ED A0 80.
fn raw_line(event: &Json) -> Vec<u8> {
    let line = write(event, Format::SPACED).unwrap();
    let (before, after) = line.split_once(SURROGATE).expect("the stand-in");
    [before.as_bytes(), b"\xed\xa0\x80", after.as_bytes()].concat()
}

/// The last line of `text` that is not blank: `_g57_tail`.
fn tail(text: &str) -> &str {
    let mut lines = text
        .trim()
        .split('\n')
        .filter(|line| !line.trim().is_empty());
    lines.next_back().unwrap_or_default()
}

mod g57_lone_surrogate_in_a_journal_is_scoped {
    use beadroll_core::json::{Format, write};
    use beadroll_test_support::{Json, Machine, parse_json};

    use super::{SURROGATE, create, escaped_line, raw_line, tail};

    /// G57: a journal line whose text holds a lone surrogate (escaped `\ud800`, or the raw bytes
    /// ED A0 80; bead itself refuses to write one) is not UTF-8 text, so it is no event: it is
    /// reported under `integrity`, its issue is nowhere, and `export -o` and `export` to standard
    /// output both work and agree. Before the fix it replayed unreported, then `export -o` died
    /// with UnicodeEncodeError and `export` to standard output wrote '?'; until roll-lqjj.53 the
    /// surrogate read as U+FFFD and the line was an event.
    #[test]
    fn export_survives_a_lone_surrogate_in_a_journal() {
        let sandbox = crate::sandbox();
        let m = Machine::new(&sandbox, "g57");
        m.bead(["init", "--prefix", "ex"]);
        m.bead(["create", "ordinary work"]);
        let escaped = create(1, "fffffff0", "ex-esc", &format!("bad{SURROGATE}title"));
        let raw = create(2, "fffffff1", "ex-raw", &format!("raw{SURROGATE}title"));
        for (replica, line) in [
            ("fffffff0", escaped_line(&escaped)),
            ("fffffff1", raw_line(&raw)),
        ] {
            // `mkdir(parents=True)`: `events/` is there, the tracker having events
            let folder = m.store().join("events").join(replica);
            std::fs::create_dir(&folder).unwrap();
            std::fs::write(folder.join("000001.jsonl"), [line, b"\n".to_vec()].concat()).unwrap();
        }
        let out = sandbox.root().join("out.jsonl");
        let proc = m.run(["export", "-o", out.to_str().unwrap(), "--json"]);
        assert!(!proc.crashed(), "export -o crashed: {}", tail(&proc.stderr));
        assert_eq!(proc.returncode(), 0, "{}", proc.stderr);
        let written = std::fs::read(&out).unwrap();
        let text = std::str::from_utf8(&written).expect("export -o wrote UTF-8");
        let mut titles: Vec<(Json, Json)> = Vec::new();
        for row in text.split('\n').filter(|row| !row.trim().is_empty()) {
            let record = parse_json(row).unwrap();
            let id = record.get("id").expect("a record's id").clone();
            let title = record.get("title").expect("a record's title");
            titles.push((id, title.clone()));
        }
        let stdout = m.run(["export"]);
        assert_eq!(
            stdout.stdout.as_bytes(),
            written,
            "export to stdout and export -o disagree"
        );
        let exported: Vec<&str> = titles.iter().filter_map(|(_, t)| t.as_str()).collect();
        assert_eq!(exported, ["ordinary work"], "{titles:?}");
        let info = m.bead(["info"]).expect("bead info prints a record");
        let integrity = info.get("integrity").expect("info's integrity");
        let rows: Vec<(String, String)> = integrity
            .as_array()
            .expect("a list")
            .iter()
            .map(|row| {
                let row = row.as_object().expect("a row");
                let text = |key: &str| match row.get(key) {
                    None => "None".to_owned(),
                    Some(Json::Str(text)) => text.clone(),
                    Some(other) => write(other, Format::ASCII).unwrap(),
                };
                (text("origin"), text("problem"))
            })
            .collect();
        for replica in ["fffffff0", "fffffff1"] {
            let place = format!("{replica}/000001.jsonl:1");
            let found = rows.iter().find(|(origin, _)| origin.contains(&place));
            let (_, problem) = found.unwrap_or_else(|| panic!("{replica} not reported: {rows:?}"));
            assert_eq!(problem, "line is not valid JSON", "{replica}");
        }
    }
}
