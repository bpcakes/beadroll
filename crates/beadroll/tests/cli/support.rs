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
    // the usual machine first: with BEAD_MIXED_WITH set, the second runs another bead
    let usual = Machine::new(&sandbox, "usual");
    let mixed = Machine::new(&sandbox, "mixed").with_bead(&own);
    assert_eq!(mixed.executable(), own);
    assert_eq!(usual.executable(), sandbox.bead());
    let run = mixed.run(["show", "ex-1"]);
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

/// The binaries `count` machines made one after another in `sandbox` run.
fn made(sandbox: &Sandbox, count: usize) -> Vec<PathBuf> {
    (0..count)
        .map(|n| {
            let machine = Machine::new(sandbox, &format!("m{n}"));
            machine.executable().to_path_buf()
        })
        .collect()
}

// The sandboxes below take the values of BEAD_UNDER_TEST and BEAD_MIXED_WITH as given, not from
// the environment, which a test cannot set for itself alone; so these tests pass whatever the
// run's own variables say.

#[test]
fn bead_mixed_with_runs_on_every_second_machine_in_the_order_they_are_made() {
    let own = Path::new(env!("CARGO_BIN_EXE_bead"));
    // A stand-in that shows it ran: echo prints its arguments.
    let second = Path::new("/bin/echo");
    let sandbox = Sandbox::selecting(own, None, Some(second.as_os_str()));
    assert_eq!((sandbox.bead(), sandbox.mixed_with()), (own, Some(second)));
    let order = made(&sandbox, 5);
    assert_eq!(order, [own, second, own, second, own]);
    // The sixth machine is a second one: it runs the stand-in, and the seventh the sandbox's bead.
    let sixth = Machine::new(&sandbox, "sixth");
    let run = sixth.run(["show", "ex-1"]);
    assert_eq!((run.returncode(), run.stdout.as_str()), (0, "show ex-1\n"));
    assert_eq!(Machine::new(&sandbox, "seventh").executable(), own);
    // BEAD_UNDER_TEST still chooses the others' bead.
    let under_test = Path::new("/usr/bin/env");
    let both = Sandbox::selecting(own, Some(under_test.as_os_str()), Some(second.as_os_str()));
    assert_eq!(made(&both, 4), [under_test, second, under_test, second]);
}

#[test]
fn a_machine_with_its_own_binary_runs_it_whatever_bead_mixed_with_says() {
    let own = Path::new(env!("CARGO_BIN_EXE_bead"));
    let (under_test, second) = (Path::new("/usr/bin/env"), Path::new("/bin/echo"));
    let sandbox = Sandbox::selecting(own, Some(under_test.as_os_str()), Some(second.as_os_str()));
    let first = Machine::new(&sandbox, "first");
    // The second and the fourth would run the second bead, the third the bead under test.
    let rust = crate::rust_only(&sandbox, "rust");
    let third = Machine::new(&sandbox, "third").with_bead("/bin/cat");
    let fourth = Machine::new(&sandbox, "fourth").with_bead(own);
    let fifth = Machine::new(&sandbox, "fifth");
    let chosen = [&first, &rust, &third, &fourth, &fifth].map(Machine::executable);
    let cat = Path::new("/bin/cat");
    assert_eq!(chosen, [under_test, own, cat, own, under_test]);
    // Overridden or not, every machine counts in the alternation: the sixth is a second one.
    assert_eq!(Machine::new(&sandbox, "sixth").executable(), second);
}

#[test]
fn an_unset_or_empty_bead_mixed_with_changes_nothing() {
    let own = Path::new(env!("CARGO_BIN_EXE_bead"));
    let under_test = Path::new("/usr/bin/env");
    for unset in [None, Some(OsStr::new(""))] {
        let sandbox = Sandbox::selecting(own, None, unset);
        assert_eq!((sandbox.bead(), sandbox.mixed_with()), (own, None));
        assert_eq!(made(&sandbox, 4), [own; 4]);
        let sandbox = Sandbox::selecting(own, Some(under_test.as_os_str()), unset);
        assert_eq!((sandbox.bead(), sandbox.mixed_with()), (under_test, None));
        assert_eq!(made(&sandbox, 4), [under_test; 4]);
    }
}
