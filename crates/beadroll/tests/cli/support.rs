//! Which `bead` the tests run, checked from the test root that supplies the default.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use beadroll_test_support::{Machine, Sandbox, UNDER_TEST};

#[test]
fn machines_run_bead_under_test_or_else_the_binary_cargo_built() {
    let expected = std::env::var_os(UNDER_TEST)
        .filter(|value| !value.is_empty())
        .map_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_bead")), PathBuf::from);
    let sandbox = crate::sandbox();
    assert_eq!(sandbox.bead(), expected);
    assert_eq!(Machine::new(&sandbox, "a").executable(), expected);
}

#[test]
fn a_machine_with_its_own_binary_runs_it_beside_the_others() {
    let sandbox = crate::sandbox();
    // A stand-in that shows it ran: echo prints its arguments.
    let own = sandbox.root().join("own-bead");
    std::os::unix::fs::symlink("/bin/echo", &own).unwrap();
    let usual = Machine::new(&sandbox, "usual");
    let other = Machine::new(&sandbox, "other").with_bead(&own);
    assert_eq!(other.executable(), own);
    assert_eq!(usual.executable(), sandbox.bead());
    let run = other.run(["show", "ex-1"]);
    assert_eq!((run.returncode(), run.stdout.as_str()), (0, "show ex-1\n"));
}

#[test]
fn a_machine_runs_in_its_checkout_with_its_own_identity_and_nothing_inherited() {
    let sandbox = crate::sandbox();
    let env = sandbox.root().join("env");
    std::os::unix::fs::symlink("/usr/bin/env", &env).unwrap();
    let machine = Machine::new(&sandbox, "m").with_bead(&env);
    let shown = machine.cmd(["pwd"]).run();
    assert_eq!(shown.returncode(), 0, "{}", shown.stderr);
    // `env pwd` prints the directory it ran in.
    assert_eq!(
        PathBuf::from(shown.stdout.trim()),
        machine.checkout().canonicalize().unwrap()
    );
    let listed = machine
        .cmd(Vec::<&str>::new())
        .thread("t-2")
        .env("BEAD_ROTATE_BYTES", "1")
        .run();
    let vars: Vec<&str> = listed.stdout.lines().collect();
    let home = format!("BEAD_HOME={}", machine.home().display());
    for wanted in [
        home.as_str(),
        "BEAD_AGENT=codex",
        "BEAD_THREAD=t-2",
        "BEAD_ROTATE_BYTES=1",
    ] {
        assert!(vars.contains(&wanted), "{wanted} not in {vars:?}");
    }
    let bead_vars: Vec<&&str> = vars.iter().filter(|v| v.starts_with("BEAD_")).collect();
    assert_eq!(bead_vars.len(), 4, "{bead_vars:?}");
    assert!(
        !vars
            .iter()
            .any(|v| v.starts_with("GIT_") || v.starts_with("CLAUDE") || v.starts_with("CODEX"))
    );
}

#[test]
fn a_rust_only_machine_runs_the_binary_cargo_built_whatever_bead_under_test_says() {
    let sandbox = crate::sandbox();
    let machine = crate::rust_only(&sandbox, "rust");
    assert_eq!(
        machine.executable(),
        PathBuf::from(env!("CARGO_BIN_EXE_bead"))
    );
}

// The sandboxes below take the value of BEAD_UNDER_TEST as given, not from the environment, which
// a test cannot set for itself alone; so these tests pass whatever the run's own variables say.

#[test]
fn bead_under_test_names_every_machines_bead_unless_a_machine_has_its_own() {
    let own = Path::new(env!("CARGO_BIN_EXE_bead"));
    let under_test = Path::new("/usr/bin/env");
    let sandbox = Sandbox::selecting(own, Some(under_test.as_os_str()));
    assert_eq!(sandbox.bead(), under_test);
    let first = Machine::new(&sandbox, "first");
    let rust = crate::rust_only(&sandbox, "rust");
    let third = Machine::new(&sandbox, "third").with_bead("/bin/cat");
    let chosen = [&first, &rust, &third].map(Machine::executable);
    assert_eq!(chosen, [under_test, own, Path::new("/bin/cat")]);
    for unset in [None, Some(OsStr::new(""))] {
        let sandbox = Sandbox::selecting(own, unset);
        assert_eq!(sandbox.bead(), own);
        assert_eq!(Machine::new(&sandbox, "m").executable(), own);
    }
}
