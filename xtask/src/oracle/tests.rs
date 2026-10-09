//! The oracle's own tests: its parsing, and the runner and the guard against a scratch suite.

use super::*;
use std::fs;

#[test]
fn every_test_module_is_fast_or_slow() {
    let root = workspace::root();
    assert_eq!(unlisted_modules(&root).unwrap(), Vec::<String>::new());
    let all = workspace::test_modules(&root).unwrap();
    assert_eq!(all.len(), FAST.len() + SLOW.len(), "{all:?}");
}

#[test]
fn summaries_are_read_whole_or_not_at_all() {
    let line =
        "test_x ran=3 passed=1 failed=1 errors=1 skipped=0 excluded=2 fixture_errors=0 seconds=4";
    let summary = Summary::parse("test_x", line).unwrap();
    assert_eq!(
        (
            summary.ran,
            summary.passed,
            summary.excluded,
            summary.seconds
        ),
        (3, 1, 2, 4)
    );
    assert_eq!(Summary::parse("test_y", line), None);
    assert_eq!(Summary::parse("test_x", "test_x ran=3 passed=1"), None);
    assert_eq!(
        Summary::parse("test_x", &line.replace("ran=3", "ran=x")),
        None
    );
}

#[test]
fn options_default_to_the_fast_modules_and_check_their_values() {
    let options = parse(vec![]).unwrap();
    assert_eq!(options.modules, FAST);
    assert!(!options.report && options.jobs == 1 && options.bead.is_none());
    let stress = parse(vec!["--stress".into(), "--jobs=3".into()]).unwrap();
    assert_eq!(stress.modules.len(), 10);
    assert_eq!(stress.jobs, 3);
    let named = parse(vec![
        "--bead".into(),
        "./bead".into(),
        "--report".into(),
        "test_replay".into(),
    ])
    .unwrap();
    assert_eq!(named.bead, Some(PathBuf::from("./bead")));
    assert_eq!(named.modules, ["test_replay"]);
    for bad in [
        &["--jobs", "0"][..],
        &["--bead"],
        &["--frob"],
        &["--stress", "test_replay"],
    ] {
        let args = bad.iter().map(OsString::from).collect();
        assert!(matches!(parse(args), Err(Failure::Usage(_))), "{bad:?}");
    }
}

/// A scratch suite: a `tests` package with one module whose tests run the binary under test
/// (`echo`) or this checkout's Python launcher directly, bypassing `BEAD_UNDER_TEST`, and one
/// whose every subtest is skipped, as a test skips where git has no translations.
fn fixture(name: &str, guard: bool) -> (Suite, PathBuf) {
    let dir = std::env::temp_dir().join(format!("xtask-oracle-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("tests")).unwrap();
    fs::write(dir.join("tests").join("__init__.py"), "").unwrap();
    let launcher = workspace::root().join("bead");
    let module = format!(
        "import os, subprocess, unittest\n\
         LAUNCHER = {launcher:?}\n\
         def run(*argv):\n    return subprocess.run(list(argv), capture_output=True, text=True)\n\
         class Fixture(unittest.TestCase):\n\
         \x20   def test_selected_binary(self):\n\
         \x20       self.assertEqual(run(os.environ['BEAD_UNDER_TEST'], 'status').stdout, 'status\\n')\n\
         \x20   def test_bypass_expecting_failure(self):\n\
         \x20       self.assertNotEqual(run(LAUNCHER, '--version').returncode, 0)\n\
         \x20   def test_bypass_expecting_success(self):\n\
         \x20       self.assertEqual(run(LAUNCHER, '--version').returncode, 0)\n\
         \x20   def test_skipped_in_subtests(self):\n\
         \x20       for locale in ('de', 'fr'):\n\
         \x20           with self.subTest(locale=locale):\n\
         \x20               self.skipTest(f'no {{locale}} translations')\n\
         \x20   def test_excluded(self):\n\
         \x20       self.fail('the skip list leaves this out')\n",
    );
    fs::write(dir.join("tests").join("test_fixture.py"), module).unwrap();
    let skip = dir.join("skip.txt");
    fs::write(&skip, "in-process test_fixture.Fixture.test_excluded\n").unwrap();
    let suite = Suite {
        runner: workspace::root()
            .join("xtask")
            .join("python")
            .join("oracle.py"),
        skip,
        bead: PathBuf::from("/bin/echo"),
        guard,
        out: dir.join("out"),
        dir: dir.clone(),
    };
    fs::create_dir_all(&suite.out).unwrap();
    (suite, dir)
}

// Needs python3 (3.11 or later, as the launcher does) on PATH, like the oracle itself.
#[test]
fn a_test_that_reaches_python_bead_instead_of_the_binary_under_test_fails() {
    let (suite, dir) = fixture("guarded", true);
    let result = run_module(&suite, "test_fixture");
    let log = fs::read_to_string(&result.log).unwrap();
    let summary = result
        .summary
        .unwrap_or_else(|| panic!("no summary: {log}"));
    assert_eq!(
        (
            summary.ran,
            summary.passed,
            summary.failed,
            summary.skipped,
            summary.excluded
        ),
        (4, 1, 2, 1, 1),
        "{log}"
    );
    assert_eq!(result.status, Some(1));
    assert!(
        log.contains("started Python bead instead of BEAD_UNDER_TEST"),
        "{log}"
    );
    let refused = fs::read_to_string(suite.out.join("test_fixture.refusals")).unwrap();
    assert_eq!(refused.lines().count(), 2, "{refused}");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn against_python_itself_the_launcher_runs_as_usual() {
    let (suite, dir) = fixture("unguarded", false);
    let result = run_module(&suite, "test_fixture");
    let log = fs::read_to_string(&result.log).unwrap();
    let summary = result
        .summary
        .unwrap_or_else(|| panic!("no summary: {log}"));
    // Python bead answers, so only the test that expected it to fail does; the test whose
    // subtests were all skipped counts as skipped, not passed.
    assert_eq!(
        (summary.ran, summary.passed, summary.failed, summary.skipped),
        (4, 2, 1, 1),
        "{log}"
    );
    assert!(!suite.out.join("test_fixture.refusals").exists());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_skip_list_naming_a_missing_test_stops_the_module() {
    let (suite, dir) = fixture("stale", true);
    fs::write(&suite.skip, "driver test_fixture.Fixture.test_gone\n").unwrap();
    let result = run_module(&suite, "test_fixture");
    assert_eq!(result.status, Some(2));
    assert!(result.summary.is_none());
    let log = fs::read_to_string(&result.log).unwrap();
    assert!(log.contains("test_fixture.Fixture.test_gone"), "{log}");
    fs::remove_dir_all(dir).unwrap();
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
