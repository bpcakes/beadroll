//! The check's own tests: its options, how each runner's output is read, and how `bead` is built.

use super::*;

#[test]
fn options_are_stress_and_real_and_nothing_else() {
    let args = |list: &[&str]| list.iter().map(OsString::from).collect::<Vec<_>>();
    assert_eq!(parse(args(&[])).ok(), Some(Options::default()));
    let both = Options {
        stress: true,
        real: true,
    };
    assert_eq!(parse(args(&["--real", "--stress"])).ok(), Some(both));
    assert!(matches!(
        parse(args(&["--full"])),
        Err(Failure::Usage(message)) if message == "unknown option --full"
    ));
}

#[test]
fn summaries_are_read_from_each_runners_output() {
    let output = |stdout: &str, stderr: &str| Output {
        status: std::process::ExitStatus::default(),
        stdout: stdout.as_bytes().to_vec(),
        stderr: stderr.as_bytes().to_vec(),
    };
    let nextest = output(
        "",
        "        PASS [   0.1s] (1/2) x\n     Summary [ 232.489s] 603 tests run: 603 passed (2 leaky), 2 skipped\n",
    );
    assert_eq!(
        summary(Kind::Nextest, &nextest).as_deref(),
        Some("603 tests run: 603 passed (2 leaky), 2 skipped")
    );
    let doctests = output(
        "test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n\
         test result: FAILED. 1 passed; 3 failed; 0 ignored; 0 measured; 0 filtered out\n",
        "",
    );
    let summed = summary(Kind::Doctests, &doctests);
    assert_eq!(summed.as_deref(), Some("3 passed, 3 failed"));
    let unittest = output("", "...\n------\nRan 3 tests in 0.012s\n\nOK\n");
    assert_eq!(
        summary(Kind::Unittest, &unittest).as_deref(),
        Some("Ran 3 tests in 0.012s OK")
    );
    assert_eq!(
        summary(Kind::Nextest, &output("error: could not compile", "")),
        None
    );
}

#[test]
fn a_step_with_nothing_to_sum_up_is_done_only_when_it_succeeded() {
    use std::os::unix::process::ExitStatusExt as _;
    let ended = |raw: i32| Output {
        status: std::process::ExitStatus::from_raw(raw),
        stdout: Vec::new(),
        stderr: b"error: boom\n".to_vec(),
    };
    assert_eq!(summary(Kind::Plain, &ended(0)).as_deref(), Some("done"));
    assert_eq!(
        summary(Kind::Plain, &ended(1 << 8)).as_deref(),
        Some("failed, exit 1")
    );
    let killed = summary(Kind::Plain, &ended(9)).expect("a summary");
    assert!(
        killed.starts_with("failed, ") && killed.contains("signal: 9"),
        "{killed}"
    );
    assert_eq!(status(&ended(2 << 8)), "exit 2, no summary");
}

#[test]
fn a_failed_nextest_run_shows_what_failed_without_the_progress() {
    let failed = Output {
        status: std::process::ExitStatus::default(),
        stdout: Vec::new(),
        stderr: b"        PASS [   0.1s] (1/2) a\n        FAIL [   0.2s] (2/2) b\n  thread 'b' panicked\n     Summary [   0.3s] 2 tests run: 1 passed, 1 failed\n".to_vec(),
    };
    let shown = diagnostics(Kind::Nextest, &failed);
    assert_eq!(shown.len(), 3, "{shown:?}");
    assert!(shown[0].contains("FAIL") && shown[1].contains("panicked"));
}

#[test]
fn bead_is_built_in_the_profile_xtask_runs_from_and_reports_its_artifacts() {
    let args = |profile: &str| {
        let command = build_command(Path::new("/repo"), Path::new(profile)).unwrap();
        assert_eq!(command.get_current_dir(), Some(Path::new("/repo")));
        command
            .get_args()
            .map(|arg| arg.to_str().unwrap().to_owned())
            .collect::<Vec<_>>()
            .join(" ")
    };
    let build =
        "build --locked --package beadroll --bin bead --message-format=json-render-diagnostics";
    assert_eq!(args("/target/aarch64-apple-darwin/debug"), build);
    assert_eq!(args("/t/release"), format!("{build} --profile release"));
    assert!(build_command(Path::new("/repo"), Path::new("/")).is_err());
}

#[test]
fn the_executable_tested_is_the_one_cargo_reports_for_bead() {
    // What `cargo build --message-format=json-render-diagnostics` prints, shortened: the library
    // has no executable, the binary has one, wherever a build target put it.
    let lib = r#"{"reason":"compiler-artifact","target":{"kind":["lib"],"crate_types":["lib"],"name":"beadroll"},"executable":null,"fresh":true}"#;
    let bin = r#"{"reason":"compiler-artifact","target":{"kind":["bin"],"crate_types":["bin"],"name":"bead"},"filenames":["/t/x86_64-unknown-linux-gnu/debug/bead"],"executable":"/t/x86_64-unknown-linux-gnu/debug/bead","fresh":false}"#;
    let done = r#"{"reason":"build-finished","success":true}"#;
    let messages = [lib, bin, done].join("\n");
    assert_eq!(
        built_executable(&messages).unwrap(),
        PathBuf::from("/t/x86_64-unknown-linux-gnu/debug/bead")
    );
    assert!(built_executable(&[lib, done].join("\n")).is_err());
    assert!(built_executable(&[bin, bin].join("\n")).is_err());
}
