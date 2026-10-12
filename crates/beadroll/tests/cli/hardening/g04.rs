//! G04 (`docs/FINDINGS.md`): re-running `init --tracked` rewrote `.beadroll.json` from scratch,
//! dropping `ref`, `where` and unknown keys (two refs, so two owners).

use beadroll_test_support::{Machine, Run, git, parse_json};

/// `_g04_commit_config`: commits `machine`'s .beadroll.json and pushes it to origin's `main`.
fn commit_config(machine: &Machine) {
    git(["add", ".beadroll.json"], Some(machine.checkout()));
    let identity = ["-c", "user.name=t", "-c", "user.email=t@example.invalid"];
    git(
        identity
            .into_iter()
            .chain(["commit", "-q", "-m", "tracker"]),
        Some(machine.checkout()),
    );
    git(
        ["push", "-q", "origin", "HEAD:main"],
        Some(machine.checkout()),
    );
}

/// `_g04_outcome`: the owner a successful claim reports, or the error of a refused one.
fn outcome(run: &Run) -> String {
    if run.returncode() != 0 {
        return run.stderr.trim().chars().take(200).collect();
    }
    let claimed = parse_json(&run.stdout).unwrap_or_else(|error| panic!("{error}: {}", run.stdout));
    let claim = claimed[0]
        .get("claim")
        .unwrap_or_else(|| panic!("no claim in {claimed:?}"));
    let owner = claim
        .get("owner")
        .unwrap_or_else(|| panic!("no owner in {claim:?}"));
    format!("confirmed owner {}", owner.as_str().expect("an owner"))
}

/// G04: `init --tracked` rebuilds .beadroll.json from scratch, keeping only workspace and remote.
mod g04_tracked_init_keeps_hand_set_config {
    use std::path::{Path, PathBuf};

    use beadroll_core::json::{Format, Object, write};
    use beadroll_test_support::{Json, Machine, git, json, parse_json};

    use super::{commit_config, outcome};

    /// `path` as text, for an argument or a JSON member.
    fn text(path: &Path) -> &str {
        path.to_str().expect("a UTF-8 path")
    }

    /// The JSON object held by path.
    fn read_object(path: &Path) -> Object {
        let read = std::fs::read_to_string(path).unwrap();
        match parse_json(&read) {
            Ok(Json::Object(members)) => *members,
            other => panic!(
                "{} is not a JSON object ({other:?}): {read}",
                path.display()
            ),
        }
    }

    /// Writes config as ASCII-escaped JSON with one-space indentation and a final newline.
    fn write_indented(path: &Path, config: Object) {
        let written = write(&Json::from(config), Format::INDENTED).expect("a config");
        std::fs::write(path, written + "\n").unwrap();
    }

    /// `config.update(members)`: each member replaced in place, or added at the end.
    fn update(config: &mut Object, members: Json) {
        let Json::Object(members) = members else {
            panic!("{members:?} is not an object");
        };
        config.extend(*members);
    }

    /// The journal segments in `machine`'s store, sorted by path.
    fn segments(machine: &Machine) -> Vec<PathBuf> {
        let replicas = std::fs::read_dir(machine.store().join("events")).unwrap();
        let mut found: Vec<PathBuf> = replicas
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.is_dir())
            .flat_map(|replica| std::fs::read_dir(replica).unwrap())
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.as_os_str().as_encoded_bytes().ends_with(b".jsonl"))
            .collect();
        found.sort();
        found
    }

    /// G04: re-running `init --tracked --remote X` keeps `"where": false`, `"ref"` and keys bead
    /// does not know; without them the next event records branch and commit again and the
    /// checkout changes ref.
    #[test]
    fn g04_reinit_keeps_where_false_ref_and_unknown_keys() {
        let sandbox = crate::sandbox();
        let m = Machine::new(&sandbox, "g04");
        let made = m.run(["init", "--prefix", "ex", "--tracked", "--json"]);
        assert_eq!(made.returncode(), 0, "{}", made.stderr);
        let config_path = m.checkout().join(".beadroll.json");
        let mut config = read_object(&config_path);
        update(
            &mut config,
            json!({"where": false, "ref": "refs/beads/team", "later_key": 1}),
        );
        write_indented(&config_path, config);
        let other = sandbox.bare("other.git");
        let again = m.run(["init", "--tracked", "--remote", text(&other), "--json"]);
        assert_eq!(again.returncode(), 0, "{}", again.stderr);
        m.bead(["create", "after re-init"]);
        let rewritten = read_object(&config_path);
        let segment = segments(&m).pop().expect("a journal segment");
        let read = std::fs::read_to_string(&segment).unwrap();
        let line = read.lines().last().expect("an event in the segment");
        let last = parse_json(line).unwrap_or_else(|error| panic!("{error}: {line}"));
        let last = last.as_object().expect("an event");
        assert_eq!(rewritten.get("remote"), Some(&Json::from(text(&other))));
        assert_eq!(
            (
                rewritten.get("where"),
                rewritten.get("ref"),
                rewritten.get("later_key")
            ),
            (
                Some(&json!(false)),
                Some(&json!("refs/beads/team")),
                Some(&json!(1))
            ),
            "init --tracked dropped hand-set keys: {rewritten:?}"
        );
        assert!(
            !last.contains_key("where"),
            "the `where: false` opt-out was dropped: the event records branch and commit"
        );
    }

    /// G04: re-init naming the same remote drops `"ref"`: this checkout moves to
    /// refs/beads/journal while its teammates stay on the committed ref, and two machines both
    /// hold an online-confirmed claim on one issue.
    #[test]
    fn g04_reinit_does_not_move_the_checkout_to_another_ref() {
        let sandbox = crate::sandbox();
        let a = Machine::new(&sandbox, "g04-a");
        let b = Machine::new(&sandbox, "g04-b");
        let origin = text(sandbox.origin());
        let made = a.run([
            "init",
            "--prefix",
            "ex",
            "--tracked",
            "--remote",
            origin,
            "--json",
        ]);
        assert_eq!(made.returncode(), 0, "{}", made.stderr);
        let config_path = a.checkout().join(".beadroll.json");
        let mut config = read_object(&config_path);
        config.insert("ref".into(), Json::from("refs/beads/team"));
        write_indented(&config_path, config);
        commit_config(&a);
        git(["pull", "-q", "origin", "main"], Some(b.checkout()));
        let created = a.bead(["create", "shared work"]).expect("the issue");
        let key = created["id"].as_str().expect("an id").to_owned();
        a.bead(["sync"]);
        b.bead(["sync"]);
        let again = a.run(["init", "--tracked", "--remote", origin, "--json"]);
        assert_eq!(again.returncode(), 0, "{}", again.stderr);
        let claims: Vec<_> = [&a, &b]
            .into_iter()
            .map(|m| {
                m.cmd(["claim", &key, "--json"])
                    .thread(format!("{}-agent", m.name()))
                    .run()
            })
            .collect();
        // Build the diagnostic before comparing the values.
        let outcomes: Vec<String> = claims.iter().map(outcome).collect();
        let mut codes: Vec<i32> = claims.iter().map(|run| run.returncode()).collect();
        codes.sort_unstable();
        assert_eq!(
            codes,
            [0, 1],
            "both machines hold a confirmed claim on the same issue: {}",
            outcomes.join(" | ")
        );
        let info = a.bead(["info"]).expect("info prints a record");
        assert_eq!(
            info.get("ref").expect("info reports the ref"),
            &json!("refs/beads/team")
        );
    }

    /// G04: re-running `init --tracked` keeps `"where": false` and, while keeping the existing
    /// keys, never writes a hand-set password back into the tracked file (guard for the fix; bead
    /// never stores one).
    #[test]
    fn g04_reinit_does_not_write_back_a_hand_set_password() {
        let sandbox = crate::sandbox();
        let m = Machine::new(&sandbox, "g04-pw");
        let made = m.run(["init", "--prefix", "ex", "--tracked", "--json"]);
        assert_eq!(made.returncode(), 0, "{}", made.stderr);
        let config_path = m.checkout().join(".beadroll.json");
        let mut config = read_object(&config_path);
        update(
            &mut config,
            json!({"remote": "https://alice:supersecret@example.invalid/x.git", "where": false}),
        );
        write_indented(&config_path, config);
        let again = m.run(["init", "--tracked", "--json"]);
        assert_eq!(again.returncode(), 0, "{}", again.stderr);
        let rewritten = Json::from(read_object(&config_path));
        let dumped = write(&rewritten, Format::ASCII).expect("a config");
        assert!(!dumped.contains("supersecret"), "{dumped}");
        assert_eq!(
            rewritten.get("where"),
            Some(&json!(false)),
            "init --tracked dropped `where`: {rewritten:?}"
        );
    }
}
