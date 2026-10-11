//! `test_hardening.V8_Python312`, rewritten as the property it stands for (`docs/RUST-PORT.md`
//! §4.6): the suite runs wherever bead is supported. The Python test ran a module with bead itself
//! on the oldest Python it supported; the Rust bead needs no Python, so what varies is the system
//! and git. Every push runs the tests on Linux and macOS, and every night under the oldest git the
//! README supports, built from source. This test keeps the workflows saying so.

/// `V8_Python312`.
mod v8_python312 {
    use regex_lite::Regex;

    use crate::workspace;

    /// The text of the file at `path` from the repository's root.
    fn read(path: &str) -> String {
        let file = workspace::root().join(path);
        std::fs::read_to_string(&file).unwrap_or_else(|error| panic!("{path}: {error}"))
    }

    #[test]
    fn the_suite_runs_on_linux_macos_and_the_oldest_supported_git() {
        let every_push = read(".github/workflows/rust.yml");
        let systems = "os: [ubuntu-latest, macos-latest]";
        assert!(
            every_push.contains(systems),
            "rust.yml's tests lost `{systems}`"
        );

        // "git 2.40+": the oldest git bead supports.
        let readme = read("README.md");
        let supported = Regex::new(r"git (\d+\.\d+)\+").expect("a valid pattern");
        let oldest = supported
            .captures(&readme)
            .map(|found| found[1].to_owned())
            .expect("the README names the oldest git it supports");

        let nightly = read(".github/workflows/rust-nightly.yml");
        let built = format!("GIT_VERSION: \"{oldest}.");
        assert!(
            nightly.contains(&built),
            "the nightly tests no git {oldest}"
        );
        assert!(
            nightly.contains("schedule:"),
            "the nightly runs on no schedule"
        );
        let checked = r#"test "$(git --version)" = "git version $GIT_VERSION""#;
        assert!(
            nightly.contains(checked),
            "the nightly does not check its git"
        );
        // The workspace's tests, on the bead Cargo builds: what each machine is meant to run.
        let tested = "cargo nextest run --workspace";
        assert!(nightly.contains(tested), "the nightly lost `{tested}`");
        assert!(
            !nightly.contains("BEAD_UNDER_TEST"),
            "the nightly tests a bead other than the one Cargo builds"
        );
    }
}
