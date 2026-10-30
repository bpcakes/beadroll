//! The coverage command's own tests: its options and steps, the tools it asks for, how its
//! summary reads a report, and a whole run on fake tools.

use std::path::PathBuf;

use super::*;
use crate::coverage::RunResult;

/// Whether a test toolchain contains the LLVM binaries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LlvmTools {
    /// Missing.
    Missing,
    /// Installed.
    Installed,
}

fn args(list: &[&str]) -> Vec<OsString> {
    list.iter().map(OsString::from).collect()
}

/// An executable shell script `name` in `dir` with `body`: its path.
fn script(dir: &Path, name: &str, body: &str) -> OsString {
    workspace::fake_tool(dir, name, body)
}

#[test]
fn options_are_stress_and_sweep_and_nothing_else() {
    assert_eq!(parse(args(&[])).ok(), Some(Options::default()));
    let both = Options {
        stress: true,
        sweep: true,
    };
    assert_eq!(parse(args(&["--sweep", "--stress"])).ok(), Some(both));
    assert!(matches!(
        parse(args(&["--real"])),
        Err(Failure::Usage(message)) if message == "unknown option --real"
    ));
}

fn setup() -> Setup {
    Setup {
        root: PathBuf::from("/repo"),
        cargo: OsString::from("cargo"),
        python: OsString::from("python3"),
        home: PathBuf::from("/out/home"),
        logs: PathBuf::from("/out/logs"),
        bead: None,
    }
}

/// A step's command line and its variables, for comparison.
fn shown(step: &Step) -> (String, String) {
    let args: Vec<String> = step
        .args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let env: Vec<String> = step
        .env
        .iter()
        .map(|(k, v)| format!("{k}={}", v.to_string_lossy()))
        .collect();
    (args.join(" "), env.join(" "))
}

#[test]
fn the_fast_suite_is_always_run_and_the_slow_ones_when_asked() {
    let names = |options| -> Vec<&'static str> {
        steps(&setup(), Path::new("/out"), options)
            .iter()
            .map(|s| s.name)
            .collect()
    };
    let reports = ["report html", "report lcov", "report json"];
    let fast: Vec<&str> = ["clean", "tests"].into_iter().chain(reports).collect();
    assert_eq!(names(Options::default()), fast);
    let slow = Options {
        stress: true,
        sweep: true,
    };
    let all: Vec<&str> = ["clean", "tests", "stress", "sweep"]
        .into_iter()
        .chain(reports)
        .collect();
    assert_eq!(names(slow), all);
}

#[test]
fn each_suite_is_the_one_check_and_the_nightly_run_and_profiles_merge() {
    let slow = Options {
        stress: true,
        sweep: true,
    };
    let steps = steps(&setup(), Path::new("/out"), slow);
    let step = |name: &str| {
        steps
            .iter()
            .find(|s| s.name == name)
            .map(shown)
            .expect("a step")
    };
    // The bead the build makes is the one counted, whatever BEAD_UNDER_TEST is in the shell.
    let profile = format!("BEAD_UNDER_TEST= LLVM_PROFILE_FILE_NAME={PROFILE}");
    assert_eq!(
        step("tests"),
        (
            "llvm-cov nextest --locked --no-report --workspace".into(),
            profile.clone()
        )
    );
    assert_eq!(
        step("stress"),
        (
            "llvm-cov nextest --locked --no-report --workspace --run-ignored only -E test(/^stress::/)".into(),
            profile.clone()
        )
    );
    // The nightly's own selection of the sweep, with all of its schedules.
    assert_eq!(
        step("sweep"),
        (
            "llvm-cov nextest --locked --no-report --package beadroll -E test(=sweep::sweep::sweep)".into(),
            format!("{profile} BEAD_SWEEP=full")
        )
    );
    // Sixteen files per binary, however many processes: a pool, not one file for each process.
    assert!(
        PROFILE.contains("%16m") && !PROFILE.contains("%p"),
        "{PROFILE}"
    );
}

#[test]
fn the_reports_leave_out_the_tooling_and_tolerate_a_profile_that_did_not_finish() {
    let steps = steps(&setup(), Path::new("/out"), Options::default());
    let report = |name: &str| {
        steps
            .iter()
            .find(|s| s.name == name)
            .map(shown)
            .expect("a step")
            .0
    };
    let common = format!("llvm-cov report --failure-mode all --ignore-filename-regex {IGNORED}");
    assert_eq!(
        report("report html"),
        format!("{common} --html --output-dir /out")
    );
    assert_eq!(
        report("report lcov"),
        format!("{common} --lcov --output-path /out/lcov.info")
    );
    assert_eq!(
        report("report json"),
        format!("{common} --json --summary-only --output-path /out/summary.json")
    );
    // The pattern is a regular expression llvm-cov reads: it names the tooling and nothing of the
    // product's.
    let ignored = regex_lite::Regex::new(IGNORED).expect("a valid pattern");
    for tooling in [
        "/r/xtask/src/check.rs",
        "/r/crates/beadroll-test-support/src/machine.rs",
        "/r/crates/beadroll-git/src/testing.rs",
    ] {
        assert!(ignored.is_match(tooling), "{tooling}");
    }
    for product in [
        "/r/crates/beadroll-sync/src/ownership.rs",
        "/r/crates/beadroll/src/main.rs",
        "/r/crates/beadroll-core/src/state/testing_note.rs",
    ] {
        assert!(!ignored.is_match(product), "{product}");
    }
}

#[test]
fn the_protocol_and_the_child_only_file_are_files_of_this_repository() {
    let root = workspace::root();
    for prefix in PROTOCOL.into_iter().chain([CHILD_ONLY]) {
        assert!(
            root.join(prefix).exists(),
            "{prefix} is not in the repository"
        );
    }
    assert!(
        std::fs::read_to_string(root.join(CHILD_ONLY))
            .expect("main.rs")
            .contains("fn main"),
        "{CHILD_ONLY} is not an executable's main"
    );
}

#[test]
fn a_log_with_a_byte_that_is_not_utf8_still_names_the_profiles_it_left_out() {
    // The checkout's path is in the line that says where the report was saved, in all three logs.
    let dir = tempfile::tempdir().expect("a scratch directory");
    let log = [
        b"warning: /t/x.profraw: invalid instrumentation profile data (file header is corrupt)\n"
            .as_slice(),
        b"    Finished report saved to /t/\xffcheckout/lcov.info\n",
    ]
    .concat();
    std::fs::write(dir.path().join("report-lcov.log"), &log).expect("a log");
    std::fs::write(dir.path().join("report-html.log"), &log).expect("a log");
    std::fs::write(
        dir.path().join("tests.log"),
        b"warning: /t/z.profraw: not a report's\n",
    )
    .expect("a log");
    assert_eq!(unmerged_profiles(dir.path()).unwrap(), ["/t/x.profraw"]);
}

#[test]
fn a_log_that_cannot_be_read_is_an_error_and_not_a_log_with_nothing_in_it() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    // a directory where a log should be: it lists as one and cannot be read as a file
    std::fs::create_dir(dir.path().join("report-lcov.log")).expect("a directory");
    let Err(Failure::Failed(message)) = unmerged_profiles(dir.path()) else {
        panic!("a log that cannot be read passed");
    };
    assert!(message.starts_with("cannot read "), "{message}");
    assert!(message.contains("report-lcov.log"), "{message}");
    let missing = dir.path().join("no-such-logs");
    assert!(matches!(
        unmerged_profiles(&missing),
        Err(Failure::Failed(_))
    ));
}

#[test]
fn a_report_step_names_the_profiles_it_left_out_and_nothing_else() {
    let log = "\
warning: /t/target/llvm-cov-target/beadroll-12273580044368410187_3.profraw: invalid instrumentation profile data (file header is corrupt)
warning: 2 functions have mismatched data
warning: /t/x.profraw: unsupported instrumentation profile format version
error: not a warning about /t/y.profraw: but an error
    Finished report saved to /t/target/coverage/lcov.info
";
    assert_eq!(
        skipped_in(log),
        [
            "/t/target/llvm-cov-target/beadroll-12273580044368410187_3.profraw",
            "/t/x.profraw"
        ]
    );
    assert!(skipped_in("").is_empty());
}

#[test]
fn the_exit_status_says_whether_the_run_passed() {
    let shown = |passed| {
        format!(
            "{:?}",
            exit_code(if passed {
                RunResult::Passed
            } else {
                RunResult::Failed
            })
        )
    };
    assert_eq!(shown(true), format!("{:?}", ExitCode::SUCCESS));
    assert_eq!(shown(false), format!("{:?}", ExitCode::FAILURE));
    assert_ne!(shown(true), shown(false));
}

/// A `bead` that was never built is the same as no tool: `cargo` prints this for a subcommand it
/// does not have.
const NO_SUBCOMMAND: &str = "echo 'error: no such command: `llvm-cov`' >&2\nexit 101\n";

/// A toolchain whose libraries are in `dir/lib` and whose LLVM tools, when `llvm_tools`, are in
/// `dir/bin`, as `rustup component add llvm-tools` puts them.
fn rustc(dir: &Path, llvm_tools: LlvmTools) -> OsString {
    let llvm_tools = matches!(llvm_tools, LlvmTools::Installed);
    std::fs::create_dir_all(dir.join("lib")).expect("the libraries");
    std::fs::create_dir_all(dir.join("bin")).expect("the binaries");
    if llvm_tools {
        for tool in ["llvm-profdata", "llvm-cov"] {
            std::fs::write(dir.join("bin").join(tool), "").expect("an LLVM tool");
        }
    }
    let libdir = dir.join("lib");
    script(dir, "rustc", &format!("echo '{}'\n", libdir.display()))
}

/// Tools in a directory of their own under `base`, `name`, so that each case has its toolchain.
fn tools(base: &Path, name: &str, cargo_body: &str, llvm_tools_mode: LlvmTools) -> Tools {
    let dir = base.join(name);
    std::fs::create_dir_all(&dir).expect("the tools' directory");
    Tools {
        cargo: script(&dir, "cargo", cargo_body),
        rustc: rustc(&dir, llvm_tools_mode),
        llvm_from_env: false,
    }
}

#[test]
fn the_tools_are_checked_and_each_missing_one_says_how_to_install_it() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let root = dir.path();
    let installed = tools(
        root,
        "installed",
        "echo 'cargo-llvm-cov 0.8.7'\n",
        LlvmTools::Installed,
    );
    assert_eq!(
        installed.require(root).unwrap(),
        "cargo-llvm-cov 0.8.7, llvm-tools"
    );

    let newer = tools(
        root,
        "newer",
        "echo 'cargo-llvm-cov 0.9.1'\n",
        LlvmTools::Installed,
    );
    let line = newer.require(root).unwrap();
    assert!(
        line.starts_with("cargo-llvm-cov 0.9.1 (the nightly job pins 0.8.7)"),
        "{line}"
    );

    let none_installed = tools(root, "none", NO_SUBCOMMAND, LlvmTools::Missing);
    let Err(Failure::Failed(message)) = none_installed.require(root) else {
        panic!("tools that are not installed passed");
    };
    assert_eq!(
        message,
        "cargo-llvm-cov is not installed: cargo install --locked cargo-llvm-cov --version 0.8.7\n\
         llvm-tools is not installed for this toolchain: rustup component add llvm-tools"
    );

    let no_llvm = tools(
        root,
        "no-llvm",
        "echo 'cargo-llvm-cov 0.8.7'\n",
        LlvmTools::Missing,
    );
    let Err(Failure::Failed(message)) = no_llvm.require(root) else {
        panic!("a toolchain without llvm-tools passed");
    };
    assert!(
        message.starts_with("llvm-tools is not installed"),
        "{message}"
    );
    assert!(!message.contains("cargo install"), "{message}");

    // LLVM_COV and LLVM_PROFDATA stand in for the toolchain's: its libraries are not looked at.
    let own_llvm = Tools {
        llvm_from_env: true,
        ..tools(
            root,
            "own",
            "echo 'cargo-llvm-cov 0.8.7'\n",
            LlvmTools::Missing,
        )
    };
    assert_eq!(
        own_llvm.require(root).unwrap(),
        "cargo-llvm-cov 0.8.7, llvm-tools"
    );
}

#[test]
fn a_cargo_that_cannot_run_is_not_one_without_llvm_cov() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let missing = Tools {
        cargo: dir.path().join("no-such-cargo").into_os_string(),
        rustc: rustc(dir.path(), LlvmTools::Installed),
        llvm_from_env: false,
    };
    let Err(Failure::Failed(message)) = missing.require(dir.path()) else {
        panic!("a cargo that does not run passed");
    };
    assert!(message.starts_with("cannot run cargo: "), "{message}");
}

#[test]
fn a_toolchain_that_cannot_run_is_not_one_without_llvm_tools() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let missing = Tools {
        cargo: script(dir.path(), "cargo", "echo 'cargo-llvm-cov 0.8.7'\n"),
        rustc: dir.path().join("no-such-rustc").into_os_string(),
        llvm_from_env: false,
    };
    let Err(Failure::Failed(message)) = missing.require(dir.path()) else {
        panic!("a toolchain that does not run passed");
    };
    assert!(message.starts_with("cannot run rustc: "), "{message}");
}

#[test]
fn percentages_round_to_a_tenth_and_say_nothing_of_a_file_without_code() {
    assert_eq!(percent(0, 0), "-");
    assert_eq!(percent(0, 7), "0.0%");
    assert_eq!(percent(7, 7), "100.0%");
    assert_eq!(percent(1, 3), "33.3%");
    assert_eq!(percent(2, 3), "66.7%");
    assert_eq!(percent(197, 207), "95.2%");
}

#[test]
fn a_file_belongs_to_its_crate_or_to_its_first_directory() {
    assert_eq!(crate_of("crates/beadroll-sync/src/tx.rs"), "beadroll-sync");
    assert_eq!(crate_of("crates/beadroll/src/main.rs"), "beadroll");
    assert_eq!(crate_of("xtask/src/main.rs"), "xtask");
    assert_eq!(crate_of("/elsewhere/b.rs"), "");
}

/// A report of these `(path, lines)`: each line `n` is `(n, hits)`.
fn report(files: &[(&str, &[(u32, u64)])]) -> String {
    let mut text = String::new();
    for (path, lines) in files {
        text += &format!("SF:/repo/{path}\n");
        for (line, hits) in *lines {
            text += &format!("DA:{line},{hits}\n");
        }
        text += "end_of_record\n";
    }
    text
}

const RAN: &[(u32, u64)] = &[(1, 3), (2, 1)];
const PARTLY: &[(u32, u64)] = &[(1, 3), (2, 0), (3, 0), (5, 1), (9, 0)];

fn summary_of(text: &str) -> (bool, String) {
    let files = lcov::parse(text, Path::new("/repo")).expect("a report");
    let mut out = Vec::new();
    let counted = summarize(&files, &mut out).expect("a summary");
    (counted, String::from_utf8(out).expect("UTF-8"))
}

#[test]
fn the_summary_gives_lines_by_crate_and_the_protocols_unreached_lines() {
    let text = report(&[
        (CHILD_ONLY, RAN),
        ("crates/beadroll-sync/src/ownership.rs", PARTLY),
        ("crates/beadroll-sync/src/link/merge.rs", RAN),
        ("crates/beadroll-core/src/json.rs", &[(1, 0)]),
    ]);
    let (counted, shown) = summary_of(&text);
    assert!(counted, "{shown}");
    // Only the protocol's files (not main.rs, not core's json), and of a file that ran in part the
    // runs of lines that never did: 2-3 and 9, not 5. A file that ran in full lists none.
    let expected = "\
lines           60.0% (6 of 10)
  beadroll                 100.0% (2 of 2)
  beadroll-core              0.0% (0 of 1)
  beadroll-sync             57.1% (4 of 7)
protocol        57.1% (4 of 7); lines that never ran:
  crates/beadroll-sync/src/ownership.rs   40.0% (2 of 5)
      2-3, 9
  crates/beadroll-sync/src/link/merge.rs 100.0% (2 of 2)
children       counted: crates/beadroll/src/main.rs ran 2 of 2 lines, which only a bead process runs
";
    assert_eq!(shown, expected);
}

#[test]
fn a_report_without_the_children_says_so() {
    let never = report(&[(CHILD_ONLY, &[(1, 0)])]);
    let (counted, shown) = summary_of(&never);
    assert!(!counted, "{shown}");
    assert!(shown.contains("children       NOT counted"), "{shown}");
    assert!(shown.contains("SCRUBBED"), "{shown}");
    let absent = report(&[("crates/beadroll-sync/src/tx.rs", RAN)]);
    assert!(!summary_of(&absent).0);
}

mod limits;
mod run;
