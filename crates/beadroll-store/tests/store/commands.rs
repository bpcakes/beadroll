//! Actor detection tests.

mod actor_detection {
    use beadroll_store::detect_actor;
    use beadroll_test_support::{Json, json};

    fn actor_with(
        vars: &[(&str, &str)],
        name: Option<&str>,
        nearest: Option<&'static str>,
    ) -> Json {
        let home = tempfile::tempdir().unwrap();
        let settings = crate::settings(home.path(), vars);
        let actor = detect_actor(&settings, name.map(std::ffi::OsStr::new), &|| nearest);
        assert!(actor.writable);
        actor.value
    }

    fn actor(vars: &[(&str, &str)], name: Option<&str>) -> Json {
        actor_with(vars, name, None)
    }

    #[test]
    fn detect_actor_environment_and_explicit_overrides() {
        assert_eq!(actor(&[], None), json!({"agent": "human"}));
        assert_eq!(
            actor(&[("CLAUDE_CODE_SESSION_ID", "claude-id")], None),
            json!({"agent": "claude", "thread": "claude-id"})
        );
        assert_eq!(
            actor(
                &[
                    ("CODEX_THREAD_ID", "thread"),
                    ("CODEX_SESSION_ID", "session")
                ],
                None
            ),
            json!({"agent": "codex", "thread": "thread"})
        );
        assert_eq!(
            actor(&[("CODEX_SESSION_ID", "session")], None),
            json!({"agent": "codex", "thread": "session"})
        );
        assert_eq!(
            actor(
                &[
                    ("CURSOR_AGENT", "1"),
                    ("CURSOR_CONVERSATION_ID", "cursor-id")
                ],
                None
            ),
            json!({"agent": "cursor", "thread": "cursor-id"})
        );
        assert_eq!(
            actor(&[("CURSOR_CONVERSATION_ID", "cursor-id")], None),
            json!({"agent": "cursor", "thread": "cursor-id"})
        );
        // `CURSOR_AGENT` alone is Cursor with no conversation id to name a thread by
        assert_eq!(
            actor(&[("CURSOR_AGENT", "1")], None),
            json!({"agent": "cursor"})
        );
        assert_eq!(
            actor(
                &[
                    ("BEAD_AGENT", "other"),
                    ("BEAD_THREAD", "stable"),
                    ("CODEX_THREAD_ID", "thread")
                ],
                Some("name")
            ),
            json!({"agent": "other", "thread": "stable", "name": "name"})
        );
        // `CLAUDECODE` alone is Claude Code with no session to name a thread by
        assert_eq!(
            actor(&[("CLAUDECODE", "1")], None),
            json!({"agent": "claude"})
        );
        // `--actor` takes precedence over `BEAD_ACTOR`
        let named =
            |given: Option<&str>| actor(&[("BEAD_ACTOR", "from-env")], given)["name"].clone();
        assert_eq!(named(None), json!("from-env"));
        assert_eq!(named(Some("from-flag")), json!("from-flag"));
    }

    #[test]
    fn bead_thread_overrides_detected_thread_without_bead_agent() {
        assert_eq!(
            actor(
                &[("BEAD_THREAD", "override"), ("CODEX_THREAD_ID", "detected")],
                None
            ),
            json!({"agent": "codex", "thread": "override"})
        );
        assert_eq!(
            actor(
                &[
                    ("BEAD_THREAD", "override"),
                    ("CURSOR_CONVERSATION_ID", "detected")
                ],
                None
            ),
            json!({"agent": "cursor", "thread": "override"})
        );
    }

    #[test]
    fn nested_harnesses_pick_the_nearest_matching_ancestor_or_fallback() {
        let claude_and_cursor = [
            ("CLAUDE_CODE_SESSION_ID", "claude-id"),
            ("CURSOR_CONVERSATION_ID", "cursor-id"),
        ];
        assert_eq!(
            actor_with(&claude_and_cursor, None, Some("cursor")),
            json!({"agent": "cursor", "thread": "cursor-id"})
        );
        assert_eq!(
            actor_with(&claude_and_cursor, None, Some("claude")),
            json!({"agent": "claude", "thread": "claude-id"})
        );
        // A harness absent from the environment is ignored; fallback prefers claude over cursor.
        assert_eq!(
            actor_with(&claude_and_cursor, None, Some("codex")),
            json!({"agent": "claude", "thread": "claude-id"})
        );
        assert_eq!(
            actor_with(&claude_and_cursor, None, None),
            json!({"agent": "claude", "thread": "claude-id"})
        );

        let codex_and_cursor = [
            ("CODEX_THREAD_ID", "codex-id"),
            ("CURSOR_CONVERSATION_ID", "cursor-id"),
        ];
        assert_eq!(
            actor_with(&codex_and_cursor, None, Some("cursor")),
            json!({"agent": "cursor", "thread": "cursor-id"})
        );
        assert_eq!(
            actor_with(&codex_and_cursor, None, None),
            json!({"agent": "codex", "thread": "codex-id"})
        );
    }
}
