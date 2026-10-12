//! G02 (`docs/FINDINGS.md`): a committed `.beadroll.json` could bind a checkout to another
//! repository's URL-derived store and redirect an existing store's remote.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use beadroll_core::json::{Format, write};
use beadroll_test_support::{Json, Sandbox, git};
use sha2::{Digest as _, Sha256};

/// Spaced, ASCII-escaped JSON.
fn dumps(value: &Json) -> String {
    write(value, Format::ASCII).expect("a serializable value")
}

/// `_g02_seed_repo`: a clone of `origin` carrying a committed .beadroll.json, pushed to its origin.
fn seed_repo(sandbox: &Sandbox, name: &str, origin: &Path, config: &Json) -> PathBuf {
    let checkout = sandbox.clone(name, Some(origin));
    std::fs::write(checkout.join(".beadroll.json"), dumps(config) + "\n").unwrap();
    git(["add", "-A"], Some(&checkout));
    let identity = ["-c", "user.name=t", "-c", "user.email=t@example.invalid"];
    git(
        identity.into_iter().chain(["commit", "-q", "-m", "cfg"]),
        Some(&checkout),
    );
    git(["push", "-q", "origin", "HEAD:main"], Some(&checkout));
    checkout
}

/// `_g02_journal_refs`: the names of the refs the bare repository `bare` holds, one a line; what
/// git printed, whether or not it succeeded.
fn journal_refs(bare: &Path) -> String {
    let output = Command::new("git")
        .arg("--git-dir")
        .arg(bare)
        .args(["for-each-ref", "--format=%(refname)"])
        .stdin(Stdio::null())
        .output()
        .expect("git starts");
    String::from_utf8(output.stdout).expect("git's output is UTF-8")
}

/// SHA-256 of `data` as lowercase hexadecimal, independently computing the workspace id.
fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

mod g02_tracked_config_cannot_adopt_another_repositorys_store {
    use std::path::Path;

    use beadroll_git::normalize_remote;
    use beadroll_test_support::{Json, Machine, Run, code, json, parse_json};

    use super::{dumps, journal_refs, seed_repo, sha256_hex};

    /// The id of the issue a `bead create --json` made: `result["id"]`.
    fn id(created: Option<Json>) -> String {
        let created = created.expect("bead printed the issue it made");
        created["id"].as_str().expect("an id").to_owned()
    }

    /// The member `key` of `value`; absence fails the test.
    fn member<'v>(value: &'v Json, key: &str) -> &'v Json {
        value
            .get(key)
            .unwrap_or_else(|| panic!("no {key:?} in {value:?}"))
    }

    /// The exit status and, for a failure, the reported error code.
    fn outcome(run: &Run) -> (i32, Option<String>) {
        let status = run.returncode();
        (status, (status != 0).then(|| code(run)))
    }

    /// The first `count` Unicode scalar values of `text`.
    fn head(text: &str, count: usize) -> String {
        text.chars().take(count).collect()
    }

    /// `path` as text, for an argument or a JSON member.
    fn text(path: &Path) -> &str {
        path.to_str().expect("a UTF-8 path")
    }

    /// G02: a committed .beadroll.json naming another repository's URL-derived workspace id (plus
    /// a remote of its own) binds the checkout to that repository's existing store, reads it,
    /// publishes it to the config's remote, and leaves that store's remembered remote switched.
    #[test]
    fn foreign_derived_workspace_id_is_refused() {
        let sandbox = crate::sandbox();
        let victim = Machine::new(&sandbox, "victim");
        victim.bead(["init", "--prefix", "p"]);
        let secret = id(victim.bead(["create", "SECRET acquisition plan", "-p", "1"]));
        victim.bead(["sync"]);
        let info = victim.bead(["info"]).expect("info prints a record");
        let pid = member(&info, "workspace")
            .as_str()
            .expect("an id")
            .to_owned();
        let normalized = normalize_remote(sandbox.origin().as_os_str(), Path::new("/"));
        assert_eq!(
            pid,
            sha256_hex(normalized.as_encoded_bytes())[..16],
            "precondition: the victim's workspace id is the public URL-derived id"
        );

        let attacker_bare = sandbox.bare("attacker.git");
        let collect = sandbox.bare("collect.git");
        let hostile = json!({"workspace": (pid.as_str()), "remote": (text(&collect))});
        seed_repo(&sandbox, "hostile-seed", &attacker_bare, &hostile);
        let victim_in_m = sandbox.clone("victim-in-m", Some(&attacker_bare));

        // A routine read inside the attacker's checkout must not reach the victim's store.
        let ready = victim.cmd(["ready", "--json"]).cwd(&victim_in_m).run();
        assert_eq!(
            outcome(&ready),
            (1, Some("CONFIG".to_owned())),
            "a foreign checkout read the victim's store: rc={} out={}",
            ready.returncode(),
            head(&ready.stdout, 200)
        );
        // A routine sync must not publish the victim's journal to the config's remote.
        let sync = victim.cmd(["sync", "--json"]).cwd(&victim_in_m).run();
        assert_ne!(
            sync.returncode(),
            0,
            "sync inside the hostile checkout succeeded against the victim's store"
        );
        assert!(
            !journal_refs(&collect).contains("refs/beads/journal"),
            "the victim's journal was published to the config's own remote"
        );
        // The victim's own checkout still coordinates through its real origin.
        let origin = sandbox.origin().canonicalize().unwrap();
        assert_eq!(
            member(
                &victim.bead(["info"]).expect("info prints a record"),
                "remote"
            ),
            &Json::from(text(&origin)),
            "the victim's own store was redirected to the config's remote"
        );
        let listed = victim.bead(["list"]).expect("list prints a record");
        let issues = member(&listed, "issues").as_array().expect("a list");
        let ids: Vec<&Json> = issues.iter().map(|issue| member(issue, "id")).collect();
        assert!(ids.contains(&&Json::from(secret.as_str())), "{ids:?}");

        // Control: an `init --tracked` config (a uuid7 workspace) copied into a repository with a
        // different origin still joins the same tracker, so the fix does not block the supported
        // sharing path.
        let a = Machine::new(&sandbox, "a");
        let coordination = sandbox.bare("coordination.git");
        a.bead([
            "init",
            "--prefix",
            "ex",
            "--tracked",
            "--remote",
            text(&coordination),
        ]);
        a.bead(["create", "shared"]);
        a.bead(["sync"]);
        let elsewhere = sandbox.clone("elsewhere", Some(&sandbox.bare("another-origin.git")));
        std::fs::copy(
            a.checkout().join(".beadroll.json"),
            elsewhere.join(".beadroll.json"),
        )
        .unwrap();
        let b = Machine::new(&sandbox, "b");
        let joined = b.cmd(["sync", "--json"]).cwd(&elsewhere).run();
        assert_eq!(joined.returncode(), 0, "{}", joined.stderr);
        let listed = b.cmd(["list"]).cwd(&elsewhere).json().expect("a list");
        assert_eq!(member(&listed, "total"), &json!(1));
        let info = b.cmd(["info"]).cwd(&elsewhere).json().expect("a record");
        assert_eq!(
            member(&info, "workspace_source"),
            &json!("tracked .beadroll.json")
        );
    }

    /// G02: editing a committed .beadroll.json `remote` to a different endpoint silently
    /// republishes an existing store's journal there; the change must require consent.
    #[test]
    fn tracked_remote_change_does_not_silently_move_an_existing_store() {
        let sandbox = crate::sandbox();
        let a = Machine::new(&sandbox, "a");
        let good = sandbox.bare("good.git");
        let evil = sandbox.bare("evil.git");
        let third = sandbox.bare("third.git");
        a.bead([
            "init",
            "--prefix",
            "p",
            "--tracked",
            "--remote",
            text(&good),
        ]);
        a.bead(["create", "work"]);
        a.bead(["sync"]);
        let config_path = a.checkout().join(".beadroll.json");
        let config = parse_json(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
        let Json::Object(mut config) = config else {
            panic!("{config:?} is not an object");
        };
        config.insert("remote".into(), Json::from(text(&evil)));
        std::fs::write(&config_path, dumps(&Json::Object(config)) + "\n").unwrap();

        let sync = a.run(["sync", "--json"]);
        assert_eq!(
            outcome(&sync),
            (1, Some("CONFIG".to_owned())),
            "the store's remote was switched by an edited config without consent: {}",
            head(&sync.stdout, 200)
        );
        assert!(
            !journal_refs(&evil).contains("refs/beads/journal"),
            "the journal was republished to the config's new remote"
        );
        // Consent through an explicit --remote is still honoured.
        assert_eq!(
            a.run(["sync", "--remote", text(&evil), "--json"])
                .returncode(),
            0,
            "an explicit --remote switch was refused"
        );
        // So is the documented switch, `init --tracked --remote` (the route the CONFIG error
        // names).
        a.bead(["init", "--tracked", "--remote", text(&third)]);
        let switched = a.run(["sync", "--json"]);
        assert_eq!(
            switched.returncode(),
            0,
            "a switch made with `init --tracked --remote` was refused: {}",
            head(&switched.stderr, 300)
        );
        assert!(journal_refs(&third).contains("refs/beads/journal"));
    }
}
