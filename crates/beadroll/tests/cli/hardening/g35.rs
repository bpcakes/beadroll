//! G35: `BEAD_HOME` is used verbatim: an unexpanded `~/...` or a relative path puts the store
//! inside the current directory, that is inside the checkout, and each subdirectory gets a
//! different store.

use std::path::Path;
use std::process::Command;

/// `_g35_untracked`: what `git status --porcelain --untracked-files=all` lists in `checkout`, one
/// entry a line; nothing when git fails, as the Python helper does not check its status.
fn untracked(checkout: &Path) -> Vec<String> {
    let output = Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=all"])
        .current_dir(checkout)
        .output()
        .expect("git starts");
    let stdout = String::from_utf8(output.stdout).expect("git's output is UTF-8");
    stdout.lines().map(str::to_owned).collect()
}

mod g35_store_never_lands_in_the_checkout {
    use beadroll_test_support::{Machine, Sandbox, code, parse_json};

    use super::untracked;

    /// The class's `setUp`: machine g35, whose checkout has a subdirectory `src`.
    fn setup(sandbox: &Sandbox) -> Machine {
        let m = Machine::new(sandbox, "g35");
        std::fs::create_dir(m.checkout().join("src")).unwrap();
        m
    }

    /// G35: `BEAD_HOME=~/beads` (as settings files and service definitions pass it) creates a
    /// directory named `~` in the checkout holding the journal and the machine identity; from
    /// `src/` the tracker is missing.
    #[test]
    fn g35_tilde_bead_home_means_the_users_home() {
        let sandbox = crate::sandbox();
        let m = setup(&sandbox);
        let fake = sandbox.root().join("fakehome");
        std::fs::create_dir(&fake).unwrap();
        let made = m
            .cmd(["init", "--prefix", "ex", "--json"])
            .env("BEAD_HOME", "~/beads")
            .env("HOME", &fake)
            .run();
        assert_eq!(made.returncode(), 0, "{}", made.stderr);
        let created = m
            .cmd(["create", "from top", "--json"])
            .env("BEAD_HOME", "~/beads")
            .env("HOME", &fake)
            .run();
        assert_eq!(created.returncode(), 0, "{}", created.stderr);
        let listed = m
            .cmd(["list", "--json"])
            .cwd(m.checkout().join("src"))
            .env("BEAD_HOME", "~/beads")
            .env("HOME", &fake)
            .run();
        assert_eq!(
            untracked(m.checkout()),
            Vec::<String>::new(),
            "the store was written inside the checkout"
        );
        assert_eq!(
            listed.returncode(),
            0,
            "a subdirectory sees another store: {}",
            listed.stderr
        );
        let listed = parse_json(&listed.stdout).unwrap();
        let issues = listed["issues"].as_array().expect("the listed issues");
        let titles: Vec<&str> = issues
            .iter()
            .map(|issue| issue["title"].as_str().expect("a title"))
            .collect();
        assert_eq!(titles, ["from top"]);
        assert!(
            fake.join("beads").join("replica.json").exists(),
            "the store is not under the user's home"
        );
    }

    /// G35: `BEAD_HOME=state/beads` writes the store (journal, lock, meta, machine identity) under
    /// the current directory of each command.
    #[test]
    fn g35_relative_bead_home_never_writes_into_the_checkout() {
        let sandbox = crate::sandbox();
        let m = setup(&sandbox);
        let made = m
            .cmd(["init", "--prefix", "ex", "--json"])
            .env("BEAD_HOME", "state/beads")
            .run();
        let said = if made.stdout.is_empty() {
            &made.stderr
        } else {
            &made.stdout
        };
        assert_eq!(
            untracked(m.checkout()),
            Vec::<String>::new(),
            "the store was written inside the checkout: {said}"
        );
        if made.returncode() != 0 {
            assert_eq!(code(&made), "CONFIG", "{}", made.stderr);
        }
    }
}
