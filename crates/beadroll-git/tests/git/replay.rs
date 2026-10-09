//! `tests/test_replay.py`, the tests of the URL rules.

mod unit_test {
    use std::ffi::OsStr;
    use std::path::Path;

    use beadroll_git::normalize_remote;

    #[test]
    fn one_remote_has_one_identity_however_it_is_spelled() {
        let spellings = [
            "git@github.com:Example/Project.git",
            "https://github.com/example/project",
            "ssh://git@github.com/example/project.git",
            "https://github.com/example/project.git/",
        ];
        let normalised: std::collections::BTreeSet<_> = spellings
            .iter()
            .map(|s| normalize_remote(OsStr::new(s), Path::new("/")))
            .collect();
        assert_eq!(
            normalised.into_iter().collect::<Vec<_>>(),
            ["github.com/example/project"]
        );
    }
}
