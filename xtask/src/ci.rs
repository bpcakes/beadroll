//! CI coverage: every push tests Linux and macOS, and nightly runs include the oldest supported
//! git built from source, the growth guards timed in release and the coverage report. These checks
//! keep the workflow matrix aligned with supported environments.

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

/// The coverage report (roll-k2wy.14.5): the nightly makes it over the slow suites as well, with
/// the cargo-llvm-cov that `cargo xtask coverage` names, and keeps it, whether the suites passed or
/// not. `problems` reads the workflow as text, so that each way of spoiling the job is a test
/// against an edited copy.
mod coverage_report {
    use super::read;
    use crate::coverage::CARGO_LLVM_COV;

    const NIGHTLY: &str = ".github/workflows/rust-nightly.yml";

    /// Where the job `name` is in `workflow`: from its line to the next job's, or the end.
    fn job_range(workflow: &str, name: &str) -> Option<std::ops::Range<usize>> {
        let header = format!("  {name}:\n");
        let start = if workflow.starts_with(&header) {
            0
        } else {
            workflow.find(&format!("\n{header}"))? + 1
        };
        let mut offset = start + header.len();
        for line in workflow.get(offset..)?.split_inclusive('\n') {
            // a sibling job: two spaces in, a name, a colon
            let sibling = line.starts_with("  ")
                && !line.starts_with("   ")
                && !line.trim_start().starts_with('#')
                && line.trim_end().ends_with(':');
            if sibling {
                break;
            }
            offset += line.len();
        }
        Some(start..offset)
    }

    /// The text of the job `name`.
    fn job<'a>(workflow: &'a str, name: &str) -> Option<&'a str> {
        workflow.get(job_range(workflow, name)?)
    }

    /// The steps of a job: each from its `- ` line to the next one's.
    fn steps(job: &str) -> Vec<&str> {
        let mut starts: Vec<usize> = job
            .match_indices("\n      - ")
            .map(|(at, _)| at + 1)
            .collect();
        starts.push(job.len());
        starts
            .windows(2)
            .map(|pair| &job[pair[0]..pair[1]])
            .collect()
    }

    /// `text` without its YAML comments: a line that starts with `#`, and what follows ` #` in
    /// the others. The job is judged by what it does, not by what it says it does.
    fn uncommented(text: &str) -> String {
        let lines: Vec<&str> = text
            .lines()
            .map(|line| {
                let code = if line.trim_start().starts_with('#') {
                    ""
                } else {
                    line.split(" #").next().unwrap_or(line)
                };
                code.trim_end()
            })
            .collect();
        lines.join("\n") + "\n"
    }

    /// The value of the `key:` line of `text` that has `indent` spaces before it.
    fn value<'a>(text: &'a str, indent: usize, key: &str) -> Option<&'a str> {
        let prefix = format!("{}{key}: ", " ".repeat(indent));
        text.lines()
            .find_map(|line| line.strip_prefix(&prefix))
            .map(str::trim)
    }

    /// What is wrong with the coverage job of `workflow`, one line each; nothing when it is right.
    fn problems(workflow: &str) -> Vec<String> {
        let workflow = uncommented(workflow);
        let Some(coverage) = job(&workflow, "coverage") else {
            return vec!["the nightly has no coverage job".to_owned()];
        };
        let mut found = Vec::new();
        let order = |name: &str| workflow.find(&format!("\n  {name}:\n"));
        if !(order("oldest-git") < order("coverage") && order("coverage") < order("slow")) {
            found.push("the job is not between oldest-git and slow".to_owned());
        }
        let steps = steps(coverage);
        job_problems(coverage, &mut found);
        install_problems(&steps, &mut found);
        run_problems(&steps, &mut found);
        upload_problems(&steps, &mut found);
        found
    }

    /// What is wrong with the job as a whole: where it runs, for how long, and whether it runs.
    fn job_problems(coverage: &str, found: &mut Vec<String>) {
        // The runner the tool pins and the timing were measured on, and a bound that stops a hang.
        if value(coverage, 4, "runs-on") != Some("ubuntu-latest") {
            found.push("the job does not run on ubuntu-latest (runs-on)".to_owned());
        }
        match value(coverage, 4, "timeout-minutes").and_then(|minutes| minutes.parse::<u32>().ok())
        {
            Some(1..=120) => {}
            other => found.push(format!(
                "the job's timeout-minutes is {other:?}, not 1 to 120, the slow job's"
            )),
        }
        for key in ["if", "needs"] {
            if value(coverage, 4, key).is_some() {
                found.push(format!(
                    "the job runs only on a condition or after another (`{key}:`): the scheduled run may skip it"
                ));
            }
        }
        if coverage.contains("continue-on-error") {
            found.push("a step may fail without failing the job (continue-on-error)".to_owned());
        }
    }

    /// What is wrong with the toolchain and the tools the job installs.
    fn install_problems(steps: &[&str], found: &mut Vec<String>) {
        let tools = |step: &&str| {
            step.lines()
                .any(|line| line.trim() == "components: llvm-tools")
        };
        if !steps.iter().any(tools) {
            found.push("the toolchain has no llvm-tools".to_owned());
        }
        // The version, exactly: `cargo-llvm-cov@0.8.70` is not `cargo-llvm-cov@0.8.7`.
        let pinned = format!("cargo-llvm-cov@{CARGO_LLVM_COV}");
        let installed = (steps.iter())
            .filter_map(|step| value(step, 10, "tool"))
            .flat_map(|tools| tools.split(',').map(str::trim));
        if !installed.clone().any(|tool| tool == pinned) {
            found.push(format!(
                "it installs no `{pinned}` (exactly), the version `cargo xtask coverage` names"
            ));
        }
    }

    /// What is wrong with the step that makes the report.
    fn run_problems(steps: &[&str], found: &mut Vec<String>) {
        let Some(step) = steps.iter().find(|step| step.contains("xtask coverage")) else {
            found.push("no step makes the report".to_owned());
            return;
        };
        let run = step
            .lines()
            .find(|line| line.trim_start().starts_with("run:"));
        let run = run.unwrap_or_default();
        for part in ["cargo --locked xtask coverage", "--stress", "--sweep"] {
            if !run.contains(part) {
                found.push(format!("the report is made without `{part}`: {run}"));
            }
        }
        if run.contains("||") || run.contains(';') || run.contains('&') {
            found.push(format!(
                "the command's exit status is not the step's: {run}"
            ));
        }
        if value(step, 8, "if").is_some() {
            found.push("the report is made only on a condition".to_owned());
        }
    }

    /// What is wrong with the step that keeps the report.
    fn upload_problems(steps: &[&str], found: &mut Vec<String>) {
        let Some(step) = steps
            .iter()
            .find(|step| step.contains("actions/upload-artifact"))
        else {
            found.push("the report is not uploaded by this job".to_owned());
            return;
        };
        if !value(step, 8, "if").is_some_and(|condition| condition.contains("!cancelled()")) {
            found.push("the report is lost when a suite fails (the upload's `if:`)".to_owned());
        }
        if value(step, 10, "name") != Some("coverage-report") {
            found.push("the artifact is not named coverage-report".to_owned());
        }
        for kept in [
            "target/coverage/html",
            "target/coverage/lcov.info",
            "target/coverage/summary.json",
            "target/coverage/logs",
        ] {
            if !step.lines().any(|line| line.trim() == kept) {
                found.push(format!("the report is not kept: no `{kept}`"));
            }
        }
        let days = value(step, 10, "retention-days").and_then(|days| days.parse::<u32>().ok());
        if !days.is_some_and(|days| (1..=90).contains(&days)) {
            found.push(format!(
                "the artifact's retention-days is {days:?}, not 1 to 90"
            ));
        }
    }

    #[test]
    fn the_nightly_makes_the_report_with_pinned_tools_and_keeps_it() {
        let found = problems(&read(NIGHTLY));
        assert!(found.is_empty(), "\n{}", found.join("\n"));
    }

    /// `workflow` with `from` replaced by `to` in the job `name`, where it must be once.
    fn edited(workflow: &str, name: &str, from: &str, to: &str) -> String {
        let range = job_range(workflow, name).expect("the job");
        let text = &workflow[range.clone()];
        assert_eq!(text.matches(from).count(), 1, "{from}");
        format!(
            "{}{}{}",
            &workflow[..range.start],
            text.replace(from, to),
            &workflow[range.end..]
        )
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "a table of edits, one for each way to spoil the job"
    )]
    fn every_way_of_spoiling_the_job_is_found() {
        let nightly = read(NIGHTLY);
        let coverage = job(&nightly, "coverage").expect("the job");
        let run = "run: cargo --locked xtask coverage --stress --sweep";
        let upload = steps(coverage)
            .into_iter()
            .find(|step| step.contains("upload-artifact"))
            .expect("the upload step");
        let timeout = (coverage.lines())
            .find(|line| line.starts_with("    timeout-minutes: "))
            .expect("a timeout");
        let change = |from: &str, to: &str| edited(&nightly, "coverage", from, to);
        let moved = {
            // the upload step now ends the file, in the slow job
            let without = change(upload, "");
            format!("{}\n{upload}", without.trim_end())
        };
        let runs_on = "    runs-on: ubuntu-latest\n";
        let cancelled = "        if: ${{ !cancelled() }}\n";
        let spoiled = [
            ("exit status", change(run, &format!("{run} || true"))),
            ("exit status", change(run, &format!("{run}; true"))),
            (
                "without `--sweep`",
                change(run, "run: cargo --locked xtask coverage --stress"),
            ),
            (
                "without `--stress`",
                change(run, "run: cargo --locked xtask coverage --sweep"),
            ),
            (
                "without `cargo --locked xtask coverage`",
                change(run, "run: cargo xtask coverage --stress --sweep"),
            ),
            (
                "timeout-minutes is None",
                change(&format!("{timeout}\n"), ""),
            ),
            (
                "timeout-minutes is Some(6000)",
                change(timeout, "    timeout-minutes: 6000"),
            ),
            (
                "timeout-minutes is Some(0)",
                change(timeout, "    timeout-minutes: 0"),
            ),
            (
                "without failing the job",
                change(run, &format!("{run}\n        continue-on-error: true")),
            ),
            (
                "no llvm-tools",
                change("          components: llvm-tools\n", ""),
            ),
            (
                "installs no `cargo-llvm-cov@0.8.7` (exactly)",
                change(
                    &format!("cargo-llvm-cov@{CARGO_LLVM_COV}"),
                    &format!("cargo-llvm-cov@{CARGO_LLVM_COV}0"),
                ),
            ),
            (
                "installs no `cargo-llvm-cov@0.8.7` (exactly)",
                change(
                    &format!("cargo-llvm-cov@{CARGO_LLVM_COV}"),
                    "cargo-llvm-cov@0.0.1",
                ),
            ),
            (
                "lost when a suite fails",
                change(cancelled, "        if: ${{ success() }}\n"),
            ),
            // the condition only in a comment
            (
                "lost when a suite fails",
                change(cancelled, "        # if: ${{ !cancelled() }}\n"),
            ),
            (
                "runs only on a condition",
                change(
                    runs_on,
                    &format!(
                        "{runs_on}    if: ${{{{ github.event_name == 'workflow_dispatch' }}}}\n"
                    ),
                ),
            ),
            (
                "runs only on a condition or after another (`needs:`)",
                change(runs_on, &format!("{runs_on}    needs: oldest-git\n")),
            ),
            (
                "does not run on ubuntu-latest",
                change(runs_on, "    runs-on: macos-latest\n"),
            ),
            (
                "no `target/coverage/lcov.info`",
                change("            target/coverage/lcov.info\n", ""),
            ),
            (
                "no `target/coverage/logs`",
                change("            target/coverage/logs\n", ""),
            ),
            (
                "retention-days is None",
                change("          retention-days: 30\n", ""),
            ),
            (
                "retention-days is Some(0)",
                change("retention-days: 30", "retention-days: 0"),
            ),
            (
                "not named coverage-report",
                change("name: coverage-report", "name: report"),
            ),
            ("not uploaded by this job", moved),
        ];
        for (expected, workflow) in spoiled {
            let found = problems(&workflow);
            assert!(
                found.iter().any(|problem| problem.contains(expected)),
                "{expected:?} not found among {found:?}"
            );
        }
    }

    #[test]
    fn comments_are_not_what_a_job_does() {
        assert_eq!(
            uncommented("a: 1 # one\n# two\n  b: 2\n"),
            "a: 1\n\n  b: 2\n"
        );
        // an action's pinned commit and its version in a comment
        assert_eq!(
            uncommented("      uses: actions/checkout@de0fac # v6\n"),
            "      uses: actions/checkout@de0fac\n"
        );
        assert_eq!(
            value("    runs-on: ubuntu-latest\n", 4, "runs-on"),
            Some("ubuntu-latest")
        );
        assert_eq!(
            value("      runs-on: x\n", 4, "runs-on"),
            None,
            "a deeper key is not the job's"
        );
    }

    #[test]
    fn the_job_is_found_in_a_workflow_and_not_beyond_it() {
        let workflow = "jobs:\n  a:\n    x: 1\n    steps:\n      - run: one\n  b-c:\n    y: 2\n";
        assert_eq!(
            job(workflow, "a"),
            Some("  a:\n    x: 1\n    steps:\n      - run: one\n")
        );
        assert_eq!(job(workflow, "b-c"), Some("  b-c:\n    y: 2\n"));
        assert_eq!(job(workflow, "d"), None);
        assert_eq!(
            steps(job(workflow, "a").unwrap_or_default()),
            ["      - run: one\n"]
        );
    }
}
