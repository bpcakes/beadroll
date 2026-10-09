//! `tests/test_commands.py`, the tests of actor detection.

mod compat_audit_identity {
    use beadroll_store::detect_actor;
    use beadroll_test_support::{Json, json};

    fn actor(vars: &[(&str, &str)], name: Option<&str>) -> Json {
        let home = tempfile::tempdir().unwrap();
        let settings = crate::settings(home.path(), vars);
        detect_actor(&settings, name, &|| None)
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
    }

    #[test]
    fn bp_thread_overrides_detected_thread_without_bp_agent() {
        assert_eq!(
            actor(
                &[("BEAD_THREAD", "override"), ("CODEX_THREAD_ID", "detected")],
                None
            ),
            json!({"agent": "codex", "thread": "override"})
        );
    }
}
