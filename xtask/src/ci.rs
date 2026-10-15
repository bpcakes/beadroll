//! CI coverage: every push tests Linux and macOS, and nightly runs include the oldest supported
//! git built from source and the growth guards timed in release. These checks keep the workflow
//! matrix aligned with supported environments.

/// The text of the file at `path` from the repository's root.
fn read(path: &str) -> String {
    let file = crate::workspace::root().join(path);
    std::fs::read_to_string(&file).unwrap_or_else(|error| panic!("{path}: {error}"))
}

/// Supported platform and git-version coverage.
mod v8_supported_environments {
    use regex_lite::Regex;

    use super::read;

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

/// The growth guards only time can keep (X15): ignored by every other run, they run nightly, in
/// release and one at a time, or not at all.
mod x15_growth_guards {
    use super::read;

    #[test]
    fn the_nightly_times_the_growth_guards_in_release_one_at_a_time() {
        let nightly = read(".github/workflows/rust-nightly.yml");
        let step = nightly
            .lines()
            .find(|line| line.contains("test(/^timing::growth::/)"))
            .expect("the nightly runs no growth guard");
        for part in [
            "--release",
            "--run-ignored only",
            "--test-threads 1",
            "--package xtask",
        ] {
            assert!(
                step.contains(part),
                "the growth guards run without `{part}`: {step}"
            );
        }
    }
}
