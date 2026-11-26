//! History entries round-trip and the decoder checks the structure and bounds of its input.

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
    assert_eq!(from_cached(&cached(&entries).unwrap()), Some(entries));
    assert_eq!(from_cached(&cached(&[]).unwrap()), Some(Vec::new()));
}

/// The public single-row JSON representation remains readable alongside the binary batch codec (AU16).
#[test]
fn individual_rows_round_trip_through_their_json_arrays() {
    for row in entries() {
        let value = row.cached();
        assert_eq!(value.as_array().unwrap().len(), 10);
        assert_eq!(value[0].as_str(), Some(row.id.to_string().as_str()));
        assert_eq!(HistoryEntry::from_cached(&value), Some(row));
    }
    assert_eq!(HistoryEntry::from_cached(&Value::Array(Vec::new())), None);
}

/// The decoder accepts structurally equivalent count encodings (AU12).
#[test]
fn an_empty_history_can_have_a_redundant_count_byte() {
    assert_eq!(cached(&[]).unwrap(), [0]);
    assert_eq!(from_cached(&[0x80, 0]), Some(Vec::new()));
}

/// The reader enforces its limit on bytes supplied independently of the writer (AU13).
#[test]
fn a_field_one_container_too_deep_is_no_cache() {
    use crate::projection::codec::NESTING;

    let row = entries().remove(0);
    let base = cached(&[row]).unwrap();
    let mut r = Reader::new(&base);
    assert_eq!(r.count(), Ok(1));
    r.id().unwrap();
    r.text().unwrap();
    let agent = r.at(); // The null leaf stays in place after all the single-child containers.
    for prefix in [&[7, 1][..], &[8, 1, 1, b'k'][..]] {
        let mut bytes = base.clone();
        bytes.splice(agent..agent, prefix.repeat(NESTING));
        assert!(from_cached(&bytes).is_some());
        bytes.splice(agent..agent, prefix.iter().copied());
        assert_eq!(read_cache(&bytes), Err(Unreadable("nesting")));
        assert!(from_cached(&bytes).is_none());
    }
}

/// All four caller-built values share the reader's nesting limit (AU09).
#[test]
fn only_readable_nesting_can_be_written_to_a_history_cache() {
    use crate::projection::codec::NESTING;

    let nested = |depth, container: fn(Value) -> Value| {
        (0..depth).fold(Value::Null, |value, _| container(value))
    };
    let containers: [fn(Value) -> Value; 2] = [
        |value| Value::Array(vec![value]),
        |value| Value::from(Object::from_iter([("k".to_owned(), value)])),
    ];
    let fields: [fn(&mut HistoryEntry) -> &mut Value; 4] = [
        |row| &mut row.agent,
        |row| &mut row.thread,
        |row| &mut row.name,
        |row| &mut row.branch,
    ];
    for container in containers {
        for field in fields {
            let mut row = entries().remove(0);
            *field(&mut row) = nested(NESTING, container);
            let bytes = cached(std::slice::from_ref(&row)).unwrap();
            let decoded = from_cached(&bytes).unwrap();
            assert_eq!(cached(&decoded).unwrap(), bytes);
            let slot = field(&mut row);
            *slot = container(std::mem::replace(slot, Value::Null));
            assert!(cached(&[row]).is_none());
        }
    }
}

#[test]
fn bytes_cut_short_or_running_on_are_no_cache() {
    let bytes = cached(&entries()).unwrap();
    for cut in 0..bytes.len() {
        assert_eq!(from_cached(&bytes[..cut]), None, "cut at {cut}");
    }
    let mut longer = bytes;
    longer.push(0);
    assert_eq!(from_cached(&longer), None);
}

/// Even when the generic count fits, the decoder rejects a count that exceeds the space for
/// whole rows before reserving them (AU02). The count prefix takes space of its own.
#[test]
fn a_count_beyond_what_the_bytes_hold_is_no_cache() {
    let mut w = Writer::default();
    w.count(2);
    let mut bytes = w.finish();
    // Two minimal rows, with the final byte of the second row missing.
    bytes.resize(bytes.len() + 2 * 25 - 1, 0);
    assert_eq!(Reader::new(&bytes).count(), Ok(2));
    assert_eq!(read_cache(&bytes), Err(Unreadable("a history entry count")));
    assert_eq!(from_cached(&bytes), None);

    bytes.push(0);
    assert_eq!(from_cached(&bytes).unwrap().len(), 2);
}

/// A count of 2⁶⁴ must fail in the varint reader before it can wrap to an empty history (AU04).
#[test]
fn an_overflowing_count_is_no_cache() {
    let mut bytes = [0x80; 10];
    bytes[9] = 2;
    assert_eq!(Reader::new(&bytes).number(), Err(Unreadable("a number")));
    assert_eq!(from_cached(&bytes), None);

    let mut w = Writer::default();
    w.number(u64::MAX);
    assert_eq!(Reader::new(&w.finish()).number(), Ok(u64::MAX));
}

/// The agent field's array and its first object's members each fit the remaining byte count
/// alone. Their combined reservation must be refused before the object's keys are read (AU06).
#[test]
fn nested_containers_cannot_reserve_the_same_history_bytes_twice() {
    let members: Object = (0..64).map(|n| (n.to_string(), Value::Null)).collect();
    let mut items = vec![Value::Null; 100];
    items[0] = Value::from(members);
    let mut row = entries().remove(0);
    row.agent = Value::Array(items);
    let bytes = cached(std::slice::from_ref(&row)).unwrap();
    assert_eq!(from_cached(&bytes), Some(vec![row]));

    // Both counts fit separately, but even one value tag per slot needs 164 bytes.
    let cut = &bytes[..160];
    assert_eq!(from_cached(cut), None);
    assert_eq!(read_cache(cut), Err(Unreadable("value container counts")));
}
