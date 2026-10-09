//! `tests/test_compat.py`, the test of the coordination ref's validation.

mod compat_audit_ref_validation {
    use beadroll_core::json::{Object, Value};
    use beadroll_store::Workspace;

    #[test]
    fn ref_names_reject_code_refs_and_traversal_accept_unicode() {
        let home = tempfile::Builder::new()
            .prefix("bead-ref-validation-")
            .tempdir()
            .unwrap();
        let settings = crate::settings(home.path(), &[("BEAD_WORKSPACE", "ref-audit")]);
        let workspace = |name: &str| {
            let mut config = Object::new();
            config.insert("ref".into(), Value::from(name));
            Workspace::new(settings.clone(), "ref-audit", None, None, config).unwrap()
        };
        for name in [
            "main",
            "refs/heads/main",
            "refs/tags/v1",
            "refs/remotes/origin/main",
            "refs/beads/../foo",
            "refs/beads/bad..name",
        ] {
            let error = workspace(name).ref_name().unwrap_err();
            assert_eq!(error.code().as_str(), "CONFIG", "{name}");
        }
        for name in ["refs/beads/journal", "refs/beads/δ", "refs/tracker/journal"] {
            assert_eq!(workspace(name).ref_name().unwrap(), name);
        }
    }
}
