//! A whole run of the command on a fake `cargo`: the reports it writes, what it says of the suites,
//! and what it does when a suite fails or no report is made.

use super::*;

/// What a run of the whole command does on a fake `cargo`: nextest ends with `nextest_exit`, a
/// report is written from `lcov` (none when it is `None`, as when no profile was made), and every
/// report step warns that each of `unmerged` was left out.
struct Fake {
    dir: tempfile::TempDir,
    out: PathBuf,
    unmerged: Vec<&'static str>,
}

impl Fake {
    fn new() -> Fake {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let out = dir.path().join("target").join("coverage");
        std::fs::create_dir_all(&out).expect("the output directory");
        Fake {
            dir,
            out,
            unmerged: Vec::new(),
        }
    }

    /// Runs the command with `options`: whether it passed, standard output and standard error.
    fn run(
        &self,
        nextest_exit: i32,
        lcov: Option<&str>,
        options: Options,
    ) -> (Result<bool, Failure>, String, String) {
        let root = self.dir.path();
        let source = root.join("report.lcov");
        match lcov {
            Some(text) => {
                // The report is of files under the root it was made in.
                let text = text.replace("SF:/repo/", &format!("SF:{}/", root.display()));
                std::fs::write(&source, text).expect("the report to copy");
            }
            None => drop(std::fs::remove_file(&source)),
        }
        let warned: String = (self.unmerged.iter())
            .map(|name| {
                format!(
                    "echo 'warning: {}/{name}: invalid instrumentation profile data \
                     (file header is corrupt)' >&2\n",
                    root.display()
                )
            })
            .collect();
        let cargo = format!(
            "case \"$2\" in\n\
             --version) echo 'cargo-llvm-cov 0.8.7'; exit 0;;\n\
             clean) exit 0;;\n\
             nextest) echo '     Summary [   1.0s] 5 tests run: 5 passed, 0 skipped'; \
                      echo '        FAIL [   0.2s] (2/5) a::b' >&2; exit {nextest_exit};;\n\
             report) {warned}for last; do :; done\n\
                     case \"$*\" in\n\
                     *--lcov*) cp '{source}' \"$last\" || exit 1;;\n\
                     *--json*) echo '{{}}' > \"$last\";;\n\
                     *--html*) mkdir -p \"$last/html\" && echo '<html>' > \"$last/html/index.html\";;\n\
                     esac;;\n\
             esac\n",
            source = source.display()
        );
        let tools = tools(root, "fake", &cargo, true);
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let passed = make(&tools, root, &self.out, options, &mut out, &mut err);
        let text = |bytes: Vec<u8>| String::from_utf8(bytes).expect("UTF-8");
        (passed, text(out), text(err))
    }
}

const REPORT: &[(&str, &[(u32, u64)])] = &[
    (CHILD_ONLY, RAN),
    ("crates/beadroll-sync/src/ownership.rs", PARTLY),
];

#[test]
fn a_run_writes_the_three_reports_and_says_what_the_suites_ran() {
    let fake = Fake::new();
    let (passed, out, err) = fake.run(0, Some(&report(REPORT)), Options::default());
    assert!(passed.expect("a report"), "{out}{err}");
    let kinds: Vec<&str> = out
        .lines()
        .map(|l| l.split_whitespace().next().unwrap_or(""))
        .collect();
    assert_eq!(
        kinds[..6],
        ["tools", "clean", "tests", "report", "report", "report"],
        "{out}"
    );
    assert!(
        out.starts_with("tools          cargo-llvm-cov 0.8.7, llvm-tools\n"),
        "{out}"
    );
    assert!(
        out.contains("tests          5 tests run: 5 passed, 0 skipped"),
        "{out}"
    );
    assert!(out.contains("ownership.rs"), "{out}");
    assert!(
        out.contains("counted: crates/beadroll/src/main.rs"),
        "{out}"
    );
    assert!(
        out.contains("lcov.info, summary.json (the HTML index and summary.json count per function"),
        "{out}"
    );
    for kept in [
        "html/index.html",
        "lcov.info",
        "summary.json",
        "logs/tests.log",
        "logs/clean.log",
    ] {
        assert!(fake.out.join(kept).is_file(), "{kept} was not kept");
    }
}

#[test]
fn a_failed_suite_fails_the_run_but_the_report_is_still_made() {
    let fake = Fake::new();
    let (passed, out, _) = fake.run(100, Some(&report(REPORT)), Options::default());
    assert!(!passed.expect("a report"), "{out}");
    assert!(
        out.contains("tests          5 tests run: 5 passed"),
        "{out}"
    );
    assert!(out.contains("FAIL [   0.2s] (2/5) a::b"), "{out}");
    assert!(fake.out.join("lcov.info").is_file());
    assert!(
        out.contains("counted: crates/beadroll/src/main.rs"),
        "{out}"
    );
}

#[test]
fn a_report_that_lost_the_children_fails_the_run() {
    let fake = Fake::new();
    let (passed, out, _) = fake.run(
        0,
        Some(&report(&[("crates/beadroll-sync/src/tx.rs", RAN)])),
        Options::default(),
    );
    assert!(!passed.expect("a report"), "{out}");
    assert!(out.contains("NOT counted"), "{out}");
}

#[test]
fn a_run_that_made_no_report_is_an_error_and_an_earlier_report_does_not_stand_in() {
    let fake = Fake::new();
    std::fs::write(fake.out.join("lcov.info"), report(REPORT)).expect("an earlier report");
    let (made, out, _) = fake.run(0, None, Options::default());
    let Err(Failure::Failed(message)) = made else {
        panic!("a run without a report passed: {made:?}\n{out}");
    };
    assert!(message.starts_with("no report was made"), "{message}");
    assert!(message.contains("logs"), "{message}");
    assert!(
        !fake.out.join("lcov.info").exists(),
        "an earlier report stayed"
    );
}

#[test]
fn a_report_that_does_not_read_is_an_error_naming_the_file() {
    let fake = Fake::new();
    let (made, _, _) = fake.run(0, Some("SF:/repo/a.rs\nDA:x,1\n"), Options::default());
    let Err(Failure::Failed(message)) = made else {
        panic!("a report that does not read passed: {made:?}");
    };
    assert!(
        message.contains("lcov.info does not read, line 2"),
        "{message}"
    );
}

#[test]
fn the_slow_suites_are_run_when_asked() {
    let fake = Fake::new();
    let slow = Options {
        stress: true,
        sweep: true,
    };
    let (passed, out, err) = fake.run(0, Some(&report(REPORT)), slow);
    assert!(passed.expect("a report"), "{out}{err}");
    for suite in ["tests", "stress", "sweep"] {
        assert!(
            out.contains(&format!("{suite:<14} 5 tests run")),
            "{suite}: {out}"
        );
    }
}

#[test]
fn missing_tools_stop_the_run_before_anything_runs() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let out = dir.path().join("coverage");
    let tools = tools(dir.path(), "none", NO_SUBCOMMAND, false);
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let made = make(
        &tools,
        dir.path(),
        &out,
        Options::default(),
        &mut stdout,
        &mut stderr,
    );
    assert!(matches!(made, Err(Failure::Failed(_))), "{made:?}");
    assert!(stdout.is_empty() && !out.exists(), "something ran");
}

#[test]
fn a_profile_left_out_of_the_reports_is_said_and_fails_the_run() {
    let mut fake = Fake::new();
    fake.unmerged = vec!["beadroll-1_3.profraw"];
    let (passed, out, _) = fake.run(0, Some(&report(REPORT)), Options::default());
    assert!(!passed.expect("a report"), "{out}");
    // Three report steps warned of it; it is one profile.
    let first = format!("{}/beadroll-1_3.profraw", fake.dir.path().display());
    let said = format!(
        "profiles       1 profile could not be merged and the counts in them are missing from this report, the first {first} (target/coverage/logs/report-*.log name them all)"
    );
    assert!(out.contains(&said), "{out}");
    assert!(
        out.contains("counted: crates/beadroll/src/main.rs"),
        "the rest is told: {out}"
    );

    let mut fake = Fake::new();
    fake.unmerged = vec!["b.profraw", "a.profraw"];
    let (passed, out, _) = fake.run(0, Some(&report(REPORT)), Options::default());
    assert!(!passed.expect("a report"), "{out}");
    assert!(
        out.contains("profiles       2 profiles could not be merged"),
        "{out}"
    );
    assert!(
        out.contains("the first ") && out.contains("/a.profraw (target/coverage/logs/report-*.log"),
        "{out}"
    );
}

#[test]
fn a_run_where_every_profile_merged_says_nothing_of_profiles() {
    let fake = Fake::new();
    let (passed, out, _) = fake.run(0, Some(&report(REPORT)), Options::default());
    assert!(passed.expect("a report"), "{out}");
    assert!(!out.contains("profiles "), "{out}");
}

#[test]
fn a_step_that_fails_is_reported_failed_with_what_it_printed_and_never_as_done() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let cargo = "if [ \"$2\" = --version ]; then echo 'cargo-llvm-cov 0.8.7'; exit 0; fi\n\
                 echo 'error: boom' >&2\nexit 1\n";
    let tools = tools(dir.path(), "boom", cargo, true);
    let out = dir.path().join("coverage");
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let made = make(
        &tools,
        dir.path(),
        &out,
        Options::default(),
        &mut stdout,
        &mut stderr,
    );
    assert!(
        matches!(made, Err(Failure::Failed(_))),
        "no report: {made:?}"
    );
    let shown = String::from_utf8(stdout).expect("UTF-8");
    // However long a fake step took: whole seconds, as the runner counts them.
    let failed = regex_lite::Regex::new(r"(?m)^(.{14}) failed, exit 1 \(\d+s\)\n    error: boom$")
        .expect("a valid pattern");
    let steps: Vec<&str> = (failed.captures_iter(&shown))
        .map(|found| found.get(1).map_or("", |step| step.as_str().trim_end()))
        .collect();
    assert_eq!(
        steps,
        ["clean", "report html", "report lcov", "report json"],
        "{shown}"
    );
    assert!(
        shown.contains("tests          exit 1, no summary"),
        "{shown}"
    );
    assert!(!shown.contains("done"), "{shown}");
}
