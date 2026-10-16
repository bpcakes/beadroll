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

    /// What is wrong with the coverage job of `workflow`, one line each; nothing when it is right.
    fn problems(workflow: &str) -> Vec<String> {
        let mut found = Vec::new();
        let mut say = |problem: String| found.push(problem);
        let Some(coverage) = job(workflow, "coverage") else {
            return vec!["the nightly has no coverage job".to_owned()];
        };
        let order = |name: &str| workflow.find(&format!("\n  {name}:\n"));
        if !(order("oldest-git") < order("coverage") && order("coverage") < order("slow")) {
            say("the job is not between oldest-git and slow".to_owned());
        }
        let timeout = (coverage.lines())
            .find_map(|line| line.strip_prefix("    timeout-minutes: "))
            .and_then(|minutes| minutes.trim().parse::<u32>().ok());
        if timeout.is_none() {
            say("the job has no timeout-minutes of its own".to_owned());
        }
        if coverage.contains("continue-on-error") {
            say("a step may fail without failing the job".to_owned());
        }
        if !coverage.contains("components: llvm-tools") {
            say("the toolchain has no llvm-tools".to_owned());
        }
        let pinned = format!("cargo-llvm-cov@{CARGO_LLVM_COV}");
        if !coverage.contains(&pinned) {
            say(format!(
                "it installs no `{pinned}`, the version `cargo xtask coverage` names"
            ));
        }
        let steps = steps(coverage);
        match steps.iter().find(|step| step.contains("xtask coverage")) {
            None => say("no step makes the report".to_owned()),
            Some(step) => {
                let run = step
                    .lines()
                    .find(|line| line.trim_start().starts_with("run:"));
                let run = run.unwrap_or_default();
                for part in ["cargo --locked xtask coverage", "--stress", "--sweep"] {
                    if !run.contains(part) {
                        say(format!("the report is made without `{part}`: {run}"));
                    }
                }
                if run.contains("||") || run.contains(';') || run.contains('&') {
                    say(format!(
                        "the command's exit status is not the step's: {run}"
                    ));
                }
                if step.contains("\n        if:") {
                    say("the report is made only on a condition".to_owned());
                }
            }
        }
        match steps
            .iter()
            .find(|step| step.contains("actions/upload-artifact"))
        {
            None => say("the report is not uploaded by this job".to_owned()),
            Some(step) => {
                if !step.contains("!cancelled()") {
                    say("the report is lost when a suite fails".to_owned());
                }
                for kept in [
                    "target/coverage/html",
                    "target/coverage/lcov.info",
                    "target/coverage/summary.json",
                ] {
                    if !step.contains(kept) {
                        say(format!("the report is not kept: no `{kept}`"));
                    }
                }
            }
        }
        found
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
            ("no timeout-minutes", change(&format!("{timeout}\n"), "")),
            (
                "without failing the job",
                change(run, &format!("{run}\n        continue-on-error: true")),
            ),
            (
                "no llvm-tools",
                change("          components: llvm-tools\n", ""),
            ),
            (
                "installs no `cargo-llvm-cov@",
                change(
                    &format!("cargo-llvm-cov@{CARGO_LLVM_COV}"),
                    "cargo-llvm-cov@0.0.1",
                ),
            ),
            (
                "lost when a suite fails",
                change(
                    "        if: ${{ !cancelled() }}\n        uses: actions/upload-artifact",
                    "        uses: actions/upload-artifact",
                ),
            ),
            (
                "no `target/coverage/lcov.info`",
                change("            target/coverage/lcov.info\n", ""),
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
