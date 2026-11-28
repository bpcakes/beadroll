//! AD04: rotating at the numeric boundary never reuses or appends to a full segment.

use beadroll_core::json::Object;
use beadroll_store::{Workspace, active_segment, append_events, encode_events};
use beadroll_test_support::events::{Event, id};
use beadroll_test_support::json;

/// AD04: segment rotation exhaustion lacked storage-boundary coverage.
#[test]
fn rotation_accepts_the_last_number_and_refuses_exhaustion_without_writing() {
    for last in [i128::MAX - 1, i128::MAX] {
        let root = tempfile::tempdir().unwrap();
        let ws = Workspace::new(
            crate::settings(root.path(), &[("BEAD_ROTATE_BYTES", "1")]),
            "rotation",
            None,
            None,
            Object::new(),
        )
        .unwrap();
        let folder = ws.events_dir.join("a");
        std::fs::create_dir_all(&folder).unwrap();
        let original = Event::new(&id(1), "init")
            .data(json!({"prefix": "ex"}))
            .json();
        let bytes = encode_events(&[original]).unwrap();
        let path = folder.join(format!("{last}.jsonl"));
        std::fs::write(&path, &bytes).unwrap();
        let next = Event::new(&id(2), "create")
            .deps(&[&id(1)])
            .data(json!({"key": "ex-1"}))
            .json();
        if last == i128::MAX {
            let error = append_events(&ws, "a", [&next]).unwrap_err();
            assert_eq!(error.code().as_str(), "IO");
            assert!(
                error
                    .to_string()
                    .contains("is not a numbered journal segment")
            );
            assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 1);
        } else {
            let target = folder.join(format!("{}.jsonl", i128::MAX));
            assert_eq!(active_segment(&ws, "a").unwrap(), target);
            append_events(&ws, "a", [&next]).unwrap();
            assert_eq!(
                std::fs::read(target).unwrap(),
                encode_events(&[next]).unwrap()
            );
            assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 2);
        }
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
}

/// A new segment is named so that it sorts after the last: `999999.jsonl` rotates into nothing,
/// since `1000000.jsonl` would be read before it and its events before the ones they follow
/// (roll-9lux). The write is refused with IO, no event appended and no segment started, saying
/// how to go on; with the rotation size above the last segment's, it is appended to.
#[test]
fn a_rotation_whose_name_would_sort_before_the_last_is_refused_without_writing() {
    let root = tempfile::tempdir().unwrap();
    let settings = |rotate: &str| {
        Workspace::new(
            crate::settings(root.path(), &[("BEAD_ROTATE_BYTES", rotate)]),
            "rotation",
            None,
            None,
            Object::new(),
        )
        .unwrap()
    };
    let ws = settings("1");
    let folder = ws.events_dir.join("a");
    std::fs::create_dir_all(&folder).unwrap();
    let original = Event::new(&id(1), "init")
        .data(json!({"prefix": "ex"}))
        .json();
    let bytes = encode_events(&[original]).unwrap();
    let path = folder.join("999999.jsonl");
    std::fs::write(&path, &bytes).unwrap();
    let next = Event::new(&id(2), "create")
        .deps(&[&id(1)])
        .data(json!({"key": "ex-1"}))
        .json();
    let error = append_events(&ws, "a", [&next]).unwrap_err();
    assert_eq!(error.code().as_str(), "IO");
    let said = error.to_string();
    assert!(said.contains("1000000.jsonl would sort before"), "{said}");
    assert!(said.contains("BEAD_ROTATE_BYTES"), "{said}");
    assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 1);
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    let roomy = settings("1000000");
    assert_eq!(active_segment(&roomy, "a").unwrap(), path);
    append_events(&roomy, "a", [&next]).unwrap();
    let mut both = bytes;
    both.extend(encode_events(&[next]).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), both);
}

/// The append target is a segment the readers take: a file named `.jsonl` alone, which
/// `segment_files` passes over, is no journal's last segment, and a write goes to the first
/// numbered segment instead of to a file nobody reads.
#[test]
fn a_bare_jsonl_file_is_never_appended_to() {
    let root = tempfile::tempdir().unwrap();
    let ws = Workspace::new(
        crate::settings(root.path(), &[]),
        "rotation",
        None,
        None,
        Object::new(),
    )
    .unwrap();
    let folder = ws.events_dir.join("a");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join(".jsonl"), b"").unwrap();
    let first = folder.join("000001.jsonl");
    assert_eq!(active_segment(&ws, "a").unwrap(), first);
    let init = Event::new(&id(1), "init")
        .data(json!({"prefix": "ex"}))
        .json();
    append_events(&ws, "a", [&init]).unwrap();
    assert_eq!(
        std::fs::read(&first).unwrap(),
        encode_events(&[init]).unwrap()
    );
    assert_eq!(std::fs::read(folder.join(".jsonl")).unwrap(), b"");
}
