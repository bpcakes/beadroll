//! Supported platform and git-version coverage.

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

/// The versions of cargo-nextest `workflow` installs, each as written after its `@`.
fn nextest_installs(workflow: &str) -> Vec<String> {
    let pinned = Regex::new(r"cargo-nextest@([0-9][0-9A-Za-z.+-]*)").expect("a valid pattern");
    (pinned.captures_iter(workflow))
        .map(|found| found[1].to_owned())
        .collect()
}

/// Every job that runs the tests installs the cargo-nextest `.config/nextest.toml` requires, the
/// one version: a job left on another after a bump would run the suite under a nextest the
/// repository's settings were not written for, in one workflow and not the other.
#[test]
fn every_workflow_installs_the_nextest_the_settings_require() {
    let settings = read(".config/nextest.toml");
    let required = Regex::new(r#"(?m)^nextest-version = "([^"]+)"$"#)
        .expect("a valid pattern")
        .captures(&settings)
        .map(|found| found[1].to_owned())
        .expect(".config/nextest.toml sets nextest-version");
    for path in [
        ".github/workflows/rust.yml",
        ".github/workflows/rust-nightly.yml",
    ] {
        let installed = nextest_installs(&read(path));
        assert!(
            !installed.is_empty(),
            "{path} installs no cargo-nextest@VERSION"
        );
        for version in installed {
            assert_eq!(version, required, "{path} installs cargo-nextest@{version}");
        }
    }
}

/// An install's version is read whole, alone or among other tools.
#[test]
fn a_nextest_install_is_read_with_its_whole_version() {
    let workflow =
        "tool: cargo-nextest@0.9.130\ntool: cargo-nextest@0.9.1300,cargo-llvm-cov@0.8.7\n";
    assert_eq!(nextest_installs(workflow), ["0.9.130", "0.9.1300"]);
}
