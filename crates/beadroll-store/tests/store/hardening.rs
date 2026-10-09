//! `tests/test_hardening.py`, the tests of the store.

mod g25_first_sync_remote_is_frozen {
    use std::ffi::OsString;

    use beadroll_core::json::{Object, Value};
    use beadroll_store::Workspace;

    /// G25: a meta.json remote that is origin in another spelling (https saved, origin since
    /// switched to ssh after a token expired) never outranks origin.
    #[test]
    fn origin_remembered_by_an_older_version_is_not_preferred() {
        let root = tempfile::tempdir().unwrap();
        let state = root.path().join("home-inprocess");
        let settings = crate::settings(&state, &[]);
        let ws = Workspace::new(
            settings,
            "legacy-ws",
            Some(OsString::from("repo")),
            Some(OsString::from("git@example.invalid:org/repo.git")),
            Object::new(),
        )
        .unwrap();
        std::fs::create_dir_all(&ws.dir).unwrap();
        std::fs::write(
            ws.dir.join("meta.json"),
            r#"{"remote": "https://example.invalid/org/repo.git"}"#,
        )
        .unwrap();
        assert_eq!(
            ws.remote(None).unwrap(),
            Some(OsString::from("git@example.invalid:org/repo.git")),
            "the remembered spelling of origin outranks origin itself"
        );
        ws.save_meta(&[
            ("remote", Value::from("git@example.invalid:org/repo.git")),
            ("last_sync", Value::from("2026-01-01T00:00:00Z")),
        ])
        .unwrap();
        let meta = std::fs::read(ws.dir.join("meta.json")).unwrap();
        let meta = beadroll_store::json_text(&meta).unwrap();
        assert!(meta.get("remote").is_none(), "{meta:?}");
    }
}

mod g32_integer_fields_stay_beads_readable {
    use beadroll_core::journal::Journal;
    use beadroll_core::replay::replay;
    use beadroll_store::read_attempt;
    use beadroll_test_support::{Json, json};

    /// G32 (the guard half; the CLI half drives import, create and update): the stricter integer
    /// rule is for writes only. Events already written with a boolean priority, or an estimate of
    /// any sign or size, keep their issue and their attempt.
    #[test]
    fn writes_refuse_booleans_and_estimates_beads_cannot_read() {
        let base = "01a00000-0000-7000-8000-00000000000";
        let mut journal = Journal::new();
        let fields = [
            json!({"priority": true}),
            json!({"estimated_minutes": (-7)}),
            json!({"estimated_minutes": 1_099_511_627_776i64}),
        ];
        for (n, extra) in fields.into_iter().enumerate() {
            let id = format!("{base}{n}");
            let deps = if n == 0 {
                json!([])
            } else {
                Json::Array(vec![Json::from(format!("{base}{}", n - 1))])
            };
            let mut all = json!({"title": (format!("t{n}"))});
            if let (Json::Object(all), Json::Object(extra)) = (&mut all, extra) {
                all.extend(extra);
            }
            let event = json!({
                "v": 1, "id": (id.as_str()), "replica": "a", "deps": deps, "actor": {"agent": "t"},
                "op": "create", "issue": (id.as_str()),
                "data": {"key": (format!("ex-{n}")), "fields": all}
            });
            journal.add(event, "a", "", true);
        }
        let st = replay(journal).unwrap();
        let mut keys: Vec<&str> = st.issues().map(|i| i.key()).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["ex-0", "ex-1", "ex-2"]);
        assert!(st.integrity().is_empty());
        let folder = tempfile::tempdir().unwrap();
        let attempt = folder
            .path()
            .join("01a00000-0000-7000-8000-000000000009.json");
        let claim = r#"{"v": 1, "id": "01a00000-0000-7000-8000-000000000009", "replica": "a", "deps": ["01a00000-0000-7000-8000-000000000002"], "actor": {"agent": "t", "thread": "t1"}, "op": "claim", "issue": "01a00000-0000-7000-8000-000000000001", "data": {"generation": 1}}"#;
        let edit = r#"{"v": 1, "id": "01a00000-0000-7000-8000-00000000000a", "replica": "a", "deps": ["01a00000-0000-7000-8000-000000000009"], "actor": {"agent": "t", "thread": "t1"}, "op": "set", "issue": "01a00000-0000-7000-8000-000000000001", "data": {"fields": {"estimated_minutes": -7}}}"#;
        std::fs::write(&attempt, format!("{claim}\n{edit}\n")).unwrap();
        let mut problems = Vec::new();
        assert_eq!(
            read_attempt(&attempt, Some(&mut problems)).len(),
            2,
            "{problems:?}"
        );
    }
}

mod g57_lone_surrogate_in_a_journal_is_scoped {
    use beadroll_core::json::{Format, write};
    use beadroll_store::read_attempt;

    /// G57: an attempt file whose text holds a lone surrogate reads back as text that can be
    /// printed and exported (U+FFFD), and the bad text is reported.
    #[test]
    fn attempt_file_with_a_lone_surrogate_reads_as_text_that_encodes() {
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let id = |n: u128| {
            beadroll_core::id::EventId::from_u128((millis << 80) | (0x7 << 76) | (0b10 << 62) | n)
                .to_string()
        };
        let (claim, issue) = (id(3), id(4));
        let line = format!(
            r#"{{"v": 1, "id": "{claim}", "replica": "fffffff0", "deps": [], "actor": {{"agent": "human", "thread": "t\ud800"}}, "op": "claim", "issue": "{issue}", "data": {{"generation": 1}}}}"#
        );
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(format!("{claim}.json"));
        std::fs::write(&path, format!("{line}\n")).unwrap();
        let mut problems = Vec::new();
        let events = read_attempt(&path, Some(&mut problems));
        assert_eq!(events.len(), 1, "{problems:?}");
        let written = write(&events[0], Format::SPACED);
        assert!(
            written.is_ok(),
            "an attempt event read back holds text that cannot be written"
        );
        assert_eq!(events[0]["actor"]["thread"], "t\u{fffd}".into());
        assert!(!problems.is_empty(), "the bad text was not reported");
    }
}
