//! Storage tests: what the store writes, its readers read back.

mod rotation;

mod metadata {
    use beadroll_core::json::{self, Format, Int, Object, Value};
    use beadroll_store::Workspace;

    /// AB03: typed counts survive generic rewrites with unknown fields and insertion order.
    #[test]
    fn typed_metadata_preserves_rewrites_and_refreshes_after_replacement() {
        let root = tempfile::tempdir().unwrap();
        let ws = Workspace::new(
            crate::settings(root.path(), &[]),
            "typed",
            None,
            None,
            Object::new(),
        )
        .unwrap();
        std::fs::create_dir_all(&ws.dir).unwrap();
        let path = ws.dir.join("meta.json");
        let original = br#"{"unknown":{"keep":[1]},"published":{"a":2,"legacy":"3","bad":[1]},"name":"before"}"#;
        std::fs::write(&path, original).unwrap();
        let before = ws.metadata(true).unwrap();
        assert_eq!(
            before.published().unwrap().count("legacy"),
            Some(Int::from(3))
        );
        assert_eq!(before.published().unwrap().count("bad"), None);
        assert!(std::sync::Arc::ptr_eq(&before, &ws.metadata(true).unwrap()));

        ws.save_meta(&[("name", Value::from("after"))]).unwrap();
        let Value::Object(mut expected) = json::parse(original).unwrap().value else {
            unreachable!()
        };
        expected.insert("name".into(), Value::from("after"));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            json::write(&Value::Object(expected), Format::INDENTED).unwrap()
        );
        assert_eq!(
            ws.metadata(true).unwrap().value("name"),
            Some(&Value::from("after"))
        );
        // A replacement must invalidate the cached typed map, just as it invalidated the old map.
        let replacement = ws.dir.join("replacement");
        std::fs::write(&replacement, br#"{"published":{"a":9},"name":"next"}"#).unwrap();
        std::fs::rename(replacement, path).unwrap();
        assert_eq!(
            ws.metadata(true).unwrap().published().unwrap().count("a"),
            Some(Int::from(9))
        );
        assert_eq!(
            ws.meta(true).unwrap().get("name"),
            Some(&Value::from("next"))
        );
        assert_eq!(before.value("name"), Some(&Value::from("before")));
    }

    /// AB03: metadata shape errors still protect the remembered remote, and malformed UTF-8
    /// keeps the store diagnostic rather than adopting serde's alternate diagnostic.
    #[test]
    fn typed_metadata_retains_strict_errors_and_lenient_empty_results() {
        let root = tempfile::tempdir().unwrap();
        let ws = Workspace::new(
            crate::settings(root.path(), &[]),
            "typed",
            None,
            None,
            Object::new(),
        )
        .unwrap();
        std::fs::create_dir_all(&ws.dir).unwrap();
        for bytes in [b"[]".as_slice(), br#"{"remote":false}"#, b"{\xff}"] {
            let temp = ws.dir.join("replacement");
            std::fs::write(&temp, bytes).unwrap();
            std::fs::rename(temp, ws.dir.join("meta.json")).unwrap();
            let error = ws.metadata(true).unwrap_err().to_string();
            let diagnostic = if bytes.contains(&255) {
                beadroll_store::json_text(bytes).unwrap_err()
            } else {
                "not an object with a string remote".to_owned()
            };
            assert!(error.contains(&diagnostic), "{error}");
            assert!(ws.meta(false).unwrap().is_empty());
        }
    }
}

/// A journal's lines (roll-k2wy.14.4): events encoded as the store appends them, compact JSON a
/// line, read back by the segment and attempt readers as the events written, member for member;
/// and a segment whose last line was cut as it was appended loses only that line. The values are
/// drawn by `beadroll_test_support::values`, seeded: a failure names its seed.
mod journal_lines {
    use beadroll_core::journal::{Problem, read_attempt, read_segment};
    use beadroll_core::json::{self, Value};
    use beadroll_store::encode_events;
    use beadroll_test_support::events::{Event, id};
    use beadroll_test_support::json;
    use beadroll_test_support::values::{Rng, identical};

    fn read_back_alike(read: &[Value], written: &[Value]) -> bool {
        read.len() == written.len() && read.iter().zip(written).all(|(r, w)| identical(r, w))
    }

    #[test]
    fn a_segment_reads_back_the_values_its_lines_were_written_from() {
        for seed in 0..500 {
            let mut rng = Rng::new(seed);
            let values: Vec<Value> = (0..1 + rng.below(8)).map(|_| rng.value(4)).collect();
            let segment = encode_events(&values).unwrap();
            let mut problems: Vec<Problem> = Vec::new();
            let read = read_segment(&segment, "events/a/000001.jsonl", Some(&mut problems));
            assert!(problems.is_empty(), "seed {seed}: {problems:?}");
            assert!(read_back_alike(&read, &values), "seed {seed}");
            // Cut inside its last line, as an append interrupted leaves it: every line before that
            // one reads back as it was, and the cut line only if what is left of it is JSON.
            let last = segment.len() - 1;
            let start = segment[..last]
                .iter()
                .rposition(|b| *b == b'\n')
                .map_or(0, |at| at + 1);
            let cut = &segment[..start + rng.below(last - start)];
            let read = read_segment(cut, "events/a/000001.jsonl", Some(&mut problems));
            assert!(problems.is_empty(), "seed {seed}: {problems:?}");
            let kept = &values[..values.len() - 1];
            let tail = json::parse(&cut[start..]).ok().map(|parsed| parsed.value);
            match tail {
                Some(tail) if !cut[start..].is_empty() => {
                    assert!(read_back_alike(&read[..kept.len()], kept), "seed {seed}");
                    assert!(identical(&read[kept.len()], &tail), "seed {seed}");
                }
                _ => assert!(read_back_alike(&read, kept), "seed {seed}"),
            }
        }
    }

    #[test]
    fn an_attempt_reads_back_the_events_it_was_written_from() {
        for seed in 0..500 {
            let mut rng = Rng::new(seed);
            let events: Vec<Value> = (0..1 + rng.below(4))
                .map(|n| {
                    let data = json!({"text": (rng.text(40)), "author": (rng.text(8))});
                    let mut data = data.as_object().unwrap().clone();
                    data.extend(rng.object(3));
                    Event::new(&id(n as u64 + 2), "comment")
                        .issue(&id(1))
                        .deps(&[&id(1)])
                        .data(Value::from(data))
                        .json()
                })
                .collect();
            let attempt = encode_events(&events).unwrap();
            let mut problems: Vec<Problem> = Vec::new();
            let read = read_attempt(&attempt, "events/a/claims/x.json", Some(&mut problems));
            assert!(problems.is_empty(), "seed {seed}: {problems:?}");
            assert!(read_back_alike(&read, &events), "seed {seed}");
        }
    }
}
