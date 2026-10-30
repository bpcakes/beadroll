//! `test_compat`'s tests of the core: comment ids in the Beads export, what an import plans for
//! a file, and what its dry run says about one.

mod import_plan;
mod json_round_trip;

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
            journal.add(
                event.json(),
                "a",
                "",
                beadroll_core::journal::JournalOrder::Sequenced,
            );
        }
        assert_eq!(
            Some(exported(&journal, "first")),
            before,
            "exported existing comment id changed after another comment"
        );
    }
}

/// What `--dry-run` says of an import (roll-dih8.26): every problem the import would be refused for,
/// and what it would bring.
mod import_dry_run {
    use beadroll_core::import::dry_run::{preview, problems};
    use beadroll_core::import::{plan_import, read_records};
    use beadroll_core::journal::Journal;
    use beadroll_core::replay::replay;
    use beadroll_core::state::State;
    use beadroll_test_support::events::{Event, id};
    use beadroll_test_support::json;

    /// A tracker `ex` with issue `ex-1`, which depends on `ex-9` by key, as an import leaves it.
    fn tracker() -> State {
        let events = [
            Event::new(&id(1), "init")
                .replica("m")
                .data(json!({"prefix": "ex", "name": "ex"})),
            Event::new(&id(2), "create")
                .replica("m")
                .deps(&[&id(1)])
                .data(json!({
                    "key": "ex-1",
                    "fields": {"title": "one"},
                    "imported": true,
                    "dangling": [{"depends_on_id": "ex-9", "type": "blocks"}]
                })),
        ];
        let mut journal = Journal::new();
        for event in events {
            journal.add(
                event.json(),
                "m",
                "",
                beadroll_core::journal::JournalOrder::Sequenced,
            );
        }
        replay(journal).unwrap()
    }

    /// Whether the import itself takes `text` into `state`.
    fn imports(state: &State, text: &str) -> bool {
        read_records(text, "f")
            .and_then(|records| plan_import(state, &records))
            .is_ok()
    }

    #[test]
    fn every_problem_is_listed_and_none_only_when_the_import_goes_through() {
        let state = tracker();
        let fine = r#"{"id":"ex-2","title":"two"}"#;
        let cases = [
            (format!("{fine}\n"), 0),
            (format!("{fine}\n{{not json\n"), 1),
            // Beads memory records have no id: each is a problem (roll-dih8.16)
            (
                format!(
                    "{fine}\n{{\"_type\":\"memory\",\"key\":\"a\"}}\n{{\"_type\":\"memory\",\"key\":\"b\"}}\n"
                ),
                2,
            ),
            (format!("{fine}\n{fine}\n"), 1),
            (
                format!(
                    "{fine}\n{{\"id\":\"ex-3\",\"title\":\"x\",\"dependencies\":[{{\"depends_on_id\":\"ex-3\"}}]}}\n"
                ),
                1,
            ),
            (
                format!("{fine}\n{{\"id\":\"ex-3\",\"title\":\"x\",\"comments\":[1]}}\n"),
                1,
            ),
            // a create bead would refuse to write: the planner's check
            (
                format!("{fine}\n{{\"id\":\"ex-3\",\"title\":\"x\",\"priority\":\"high\"}}\n"),
                1,
            ),
            (
                format!(
                    "{fine}\n{{\"id\":\"ex-4\",\"title\":\"x\",\"priority\":\"high\"}}\n{{not json\n"
                ),
                2,
            ),
        ];
        for (text, count) in cases {
            let found = problems(&state, &text, "f");
            assert_eq!(
                found.is_empty(),
                imports(&state, &text),
                "{text}: {found:?}"
            );
            assert_eq!(found.len(), count, "{text}: {found:?}");
        }
        let memory = "{\"_type\":\"memory\",\"key\":\"a\"}\n";
        let found = problems(&state, memory, "issues.jsonl");
        assert_eq!(
            found[0].to_string(),
            "issues.jsonl:1: record has no string 'id'"
        );
    }

    #[test]
    fn the_preview_counts_what_is_new_known_unowned_and_waiting() {
        let state = tracker();
        let text = concat!(
            // known: left as it is, but for the dependency it no longer lists
            "{\"id\":\"ex-1\",\"title\":\"one\"}\n",
            "{\"id\":\"ex-2\",\"title\":\"two\",\"status\":\"in_progress\",\"dependencies\":",
            "[{\"depends_on_id\":\"ex-8\",\"type\":\"blocks\"},{\"depends_on_id\":\"ex-1\",\"type\":\"blocks\"}]}\n",
            "{\"id\":\"ex-3\",\"title\":\"three\",\"status\":\"closed\",\"dependencies\":",
            "[{\"depends_on_id\":\"ex-2\",\"type\":\"blocks\"},{\"depends_on_id\":\"ex-8\",\"type\":\"blocks\"}]}\n",
            "{\"id\":\"ex-4\",\"title\":\"four\"}\n",
        );
        let records = read_records(text, "f").unwrap();
        let would = preview(&state, &records);
        assert_eq!((would.records, would.known), (4, 1));
        let statuses: Vec<(&str, usize)> = (would.statuses.iter())
            .map(|(s, n)| (s.as_str(), *n))
            .collect();
        assert_eq!(statuses, [("closed", 1), ("in_progress", 1), ("open", 1)]);
        assert_eq!(would.unowned, ["ex-2"]);
        assert_eq!(would.waiting, ["ex-8"]);
        // what the import would do agrees: three issues made, ex-1's dependency on ex-9 taken back
        let plan = plan_import(&state, &records).unwrap();
        assert_eq!(plan.imported, would.records - would.known);
        assert_eq!(
            plan.steps
                .iter()
                .filter(|step| step.op() == "dep_remove")
                .count(),
            1
        );
    }
}
