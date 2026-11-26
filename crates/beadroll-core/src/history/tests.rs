//! A history cache's bytes read back as the entries written, and no other bytes read as any.

use super::*;
use crate::json::Object;

/// Entries with every kind of field: null and written actors, an object branch, changed fields,
/// a takeover, and none of them.
fn entries() -> Vec<HistoryEntry> {
    let mut branch = Object::new();
    branch.insert("name".into(), Value::from("main"));
    branch.insert(
        "depth".into(),
        Value::Array(vec![Value::from(1), Value::Null]),
    );
    let id = |n: u64| EventId::parse(&format!("0190{n:04x}-0000-7000-8000-{n:012x}")).unwrap();
    vec![
        HistoryEntry {
            id: id(1),
            machine: "a".to_owned(),
            agent: Value::Null,
            thread: Value::Null,
            name: Value::Null,
            operation: "init".to_owned(),
            issue: None,
            branch: Value::Null,
            changed_fields: Vec::new(),
            takeover_of: None,
        },
        HistoryEntry {
            id: id(2),
            machine: "b".to_owned(),
            agent: Value::from("claude"),
            thread: Value::from("t-1"),
            name: Value::from("ünïcode"),
            operation: "set".to_owned(),
            issue: Some(id(1).to_string()),
            branch: Value::Object(Box::new(branch)),
            changed_fields: vec!["title".to_owned(), "priority".to_owned()],
            takeover_of: Some(id(1).to_string()),
        },
    ]
}

#[test]
fn a_cache_reads_back_as_the_entries_written() {
    let entries = entries();
    assert_eq!(from_cached(&cached(&entries)), Some(entries));
    assert_eq!(from_cached(&cached(&[])), Some(Vec::new()));
}

#[test]
fn bytes_cut_short_or_running_on_are_no_cache() {
    let bytes = cached(&entries());
    for cut in 0..bytes.len() {
        assert_eq!(from_cached(&bytes[..cut]), None, "cut at {cut}");
    }
    let mut longer = bytes;
    longer.push(0);
    assert_eq!(from_cached(&longer), None);
}

/// Bytes that claim more entries than they can hold are no cache, the room made for the entries
/// bounded by the bytes rather than by the count they claim.
#[test]
fn a_count_beyond_what_the_bytes_hold_is_no_cache() {
    let mut w = Writer::default();
    w.count(1 << 20);
    let mut bytes = w.finish();
    // every one of them, as zeros, a readable entry, until the bytes run out
    bytes.resize(bytes.len() + (1 << 20) - 8, 0);
    assert_eq!(from_cached(&bytes), None);
}
