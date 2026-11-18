//! Actor detection tests.

mod actor_detection {
    use beadroll_store::detect_actor;
    use beadroll_test_support::{Json, json};

    fn actor(vars: &[(&str, &str)], name: Option<&str>) -> Json {
        let home = tempfile::tempdir().unwrap();
        let settings = crate::settings(home.path(), vars);
        let actor = detect_actor(&settings, name.map(std::ffi::OsStr::new), &|| None);
        assert!(actor.writable);
        actor.value
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
    }
}
