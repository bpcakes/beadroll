//! `tests/test_replay.py`, the tests of the store.

mod unit_test {
    use beadroll_core::json::Object;
    use beadroll_core::json::Value;
    use beadroll_store::Workspace;

    #[test]
    fn the_coordination_ref_cannot_be_a_branch() {
        let home = tempfile::tempdir().unwrap();
        let settings = crate::settings(home.path(), &[]);
        let workspace = |name: &str| {
            let mut config = Object::new();
            config.insert("ref".into(), Value::from(name));
            Workspace::new(settings.clone(), "w", None, None, config).unwrap()
        };
        for name in [
            "refs/heads/main",
            "refs/tags/v1",
            "main",
            "refs/beads/bad..name",
        ] {
            assert!(workspace(name).ref_name().is_err(), "{name}");
        }
        assert_eq!(
            workspace("refs/tracker/journal").ref_name().unwrap(),
            "refs/tracker/journal"
        );
    }
}

mod replay_audit_properties {
    use beadroll_core::json::Object;
    use beadroll_core::replay::replay;
    use beadroll_store::{Workspace, append_events, load_journal};
    use beadroll_test_support::events::Event;
    use beadroll_test_support::{Json, json};

    /// The first audit's history: an `init` and the issue `au-one` on machine `seed`.
    fn audit_events() -> (Vec<Json>, String) {
        let id = |n: u128| {
            let value = ((1_790_000_000_000 + n) << 80) | (7 << 76) | (2 << 62) | n;
            beadroll_core::id::EventId::from_u128(value).to_string()
        };
        let actor = json!({"agent": "codex", "thread": "seed-thread"});
        let init = Event::new(&id(1), "init")
            .replica("seed")
            .actor(actor.clone())
            .data(json!({"prefix": "au", "name": "audit"}))
            .json();
        let create = Event::new(&id(2), "create")
            .replica("seed")
            .actor(actor)
            .deps(&[&id(1)])
            .data(json!({"key": "au-one", "fields": {"title": "au-one"}}))
            .json();
        (vec![init, create], id(2))
    }

    /// The same event ids in two workspaces replay independently.
    #[test]
    fn reused_event_ids_are_isolated_across_workspaces() {
        let (events, issue) = audit_events();
        let mut other = events.clone();
        if let Some(Json::Object(last)) = other.last_mut()
            && let Some(Json::Object(data)) = last.get_mut("data")
            && let Some(Json::Object(fields)) = data.get_mut("fields")
        {
            fields.insert("title".into(), Json::from("separate workspace"));
        }
        let home = tempfile::Builder::new()
            .prefix("bead-audit-workspaces-")
            .tempdir()
            .unwrap();
        let settings = crate::settings(home.path(), &[("BEAD_WORKSPACE", "audit-explicit")]);
        let one = Workspace::new(settings.clone(), "one", None, None, Object::new()).unwrap();
        let two = Workspace::new(settings, "two", None, None, Object::new()).unwrap();
        append_events(&one, "seed", &events).unwrap();
        append_events(&two, "seed", &other).unwrap();
        let states: Vec<_> = [&one, &two]
            .iter()
            .map(|ws| replay(load_journal(ws).unwrap()).unwrap())
            .collect();
        let titles: Vec<&str> = states
            .iter()
            .map(|st| st.issue_named(&issue).unwrap().title())
            .collect();
        assert_eq!(titles, ["au-one", "separate workspace"]);
        assert!(states.iter().all(|st| st.integrity().is_empty()));
    }
}
