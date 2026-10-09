//! The replay halves of `test_commands`' tests whose other half drives the CLI.

mod review6 {
    use beadroll_core::journal::Journal;
    use beadroll_core::replay::replay;
    use beadroll_test_support::events::Event;
    use beadroll_test_support::json;

    /// F09: a dependency removed before its target exists does not reappear, and does not cycle
    /// (the replay half: the removal names the target's future uid, and the create comes after).
    #[test]
    fn f09_dangling_dependency_can_be_retracted_before_its_target_exists() {
        let create_a = "01900000-0000-7000-8000-000000000001";
        let remove = "01900000-0000-7000-8000-000000000002";
        let create_b = "01900000-0000-7000-8000-000000000003";
        let fields = |title: &str| json!({"title": title, "status": "open", "priority": 2, "issue_type": "task"});
        let events = [
            Event::new(create_a, "create").replica("m").data(json!({
                "key": "ex-a",
                "fields": (fields("A")),
                "dangling": [{"depends_on_id": "ex-b", "type": "blocks"}]
            })),
            Event::new(remove, "dep_remove")
                .replica("m")
                .deps(&[create_a])
                .issue(create_a)
                .data(json!({"to": create_b, "type": "blocks"})),
            Event::new(create_b, "create")
                .replica("m")
                .deps(&[remove])
                .data(json!({"key": "ex-b", "fields": (fields("B"))})),
        ];
        let mut journal = Journal::new();
        for event in events {
            journal.add(event.json(), "m", "", true);
        }
        let state = replay(journal).unwrap();
        let a = state.issue_named(create_a).unwrap();
        assert!(a.deps().is_empty(), "{:?}", a.deps());
        assert!(!state.is_blocked(a));
    }
}
