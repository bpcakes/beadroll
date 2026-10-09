//! `test_compat`'s test of the core: comment ids in the Beads export.

mod compat_audit_comment_ids {
    use beadroll_core::journal::Journal;
    use beadroll_core::json::Value;
    use beadroll_core::replay::replay;
    use beadroll_test_support::events::{Event, id_at};
    use beadroll_test_support::json;

    /// The exported id of a comment, by its text.
    fn exported(journal: &Journal, text: &str) -> Value {
        let state = replay(journal.clone()).unwrap();
        let records = state.export_records();
        let comments = records
            .first()
            .and_then(|record| record.get("comments"))
            .cloned()
            .unwrap();
        let comment = comments
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["text"] == Value::from(text))
            .cloned();
        comment.unwrap()["id"].clone()
    }

    /// An exported comment's id stays what it was after a later comment whose clock went back.
    #[test]
    fn existing_comment_export_id_survives_clock_skewed_new_comment() {
        let (create, first, later) = (id_at(1000, 0), id_at(3000, 0), id_at(2000, 0));
        let events = [
            (
                create.clone(),
                vec![],
                "create",
                json!({"key": "ex-a", "fields": {"title": "a"}}),
            ),
            (
                first.clone(),
                vec![create.as_str()],
                "comment",
                json!({"text": "first"}),
            ),
            (
                later.clone(),
                vec![first.as_str()],
                "comment",
                json!({"text": "later clock moved back"}),
            ),
        ];
        let mut journal = Journal::new();
        let mut before = None;
        for (eid, deps, op, data) in events {
            if eid == later {
                before = Some(exported(&journal, "first"));
            }
            let event = Event::new(&eid, op)
                .deps(&deps)
                .issue(&create)
                .actor(json!({"agent": "audit"}))
                .data(data);
            journal.add(event.json(), "a", "", true);
        }
        assert_eq!(
            Some(exported(&journal, "first")),
            before,
            "exported existing comment id changed after another comment"
        );
    }
}
