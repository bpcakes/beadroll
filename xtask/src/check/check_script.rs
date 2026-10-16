//! The baseline fails if a selected step fails. Fake cargo and python3 executables produce
//! synthetic statuses, round-trip details and summaries, and failure diagnostics on stderr.

use super::*;

/// A check with `--real` whose test steps exit with `module_status` and whose round trip exits
/// with `real_status`: whether it passed, and what it printed on each stream.
fn check(module_status: i32, real_status: i32) -> (bool, String, String) {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let verdict = if module_status == 0 { "OK" } else { "FAILED" };
    let cargo = script(
        dir.path(),
        "cargo",
        &format!(
            "echo '     Summary [   0.001s] 1 test run: 1 passed'\n\
             echo 'test result: ok. 1 passed; 0 failed'\n\
             exit {module_status}\n"
        ),
    );
    let python = script(
        dir.path(),
        "python3",
        &format!(
            "if [ \"$1\" = \"-m\" ]; then\n\
             echo 'Ran 1 test in 0.001s'\necho {verdict}\nexit {module_status}\nfi\n\
             echo 'roundtrip details'\necho 'roundtrip summary'\n\
             if [ {real_status} -ne 0 ]; then echo 'roundtrip diagnostic' >&2; fi\n\
             exit {real_status}\n"
        ),
    );
    let setup = Setup {
        root: dir.path().to_path_buf(),
        cargo,
        python,
        home: dir.path().join("home"),
        logs: dir.path().to_path_buf(),
        bead: Some(dir.path().join("bead")),
    };
    let options = Options {
        stress: false,
        real: true,
    };
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let passed = run(&steps(&setup, options), &setup, &mut out, &mut err).expect("a report");
    let text = |bytes: Vec<u8>| String::from_utf8(bytes).expect("UTF-8");
    (passed, text(out), text(err))
}

/// An executable shell script `name` in `dir` with `body`: its path.
fn script(dir: &Path, name: &str, body: &str) -> OsString {
    workspace::fake_tool(dir, name, body)
}

#[test]
fn success_keeps_the_roundtrip_summary() {
    let (passed, out, err) = check(0, 0);
    assert!(passed, "{out}{err}");
    assert_eq!(out.lines().last(), Some("roundtrip summary"), "{out}");
    assert!(!out.contains("roundtrip details"), "{out}");
}

#[test]
fn roundtrip_failure_fails_the_check_and_keeps_diagnostics() {
    let (passed, out, err) = check(0, 17);
    assert!(!passed, "{out}");
    assert!(err.contains("roundtrip diagnostic"), "{err}");
    assert!(out.contains("roundtrip details"), "{out}");
}

#[test]
fn successful_roundtrip_does_not_hide_a_failed_module() {
    let (passed, out, _) = check(19, 0);
    assert!(!passed, "{out}");
    assert!(out.contains("roundtrip summary"), "{out}");
}
