//! `PortAlignment`, from the Rust port's Phase 1 (`roll-k2wy.2`): input CPython read by accident is
//! refused by every bead. U05 (`docs/FINDINGS.md`; `docs/RUST-PORT.md` §5 row 23): the torn-tail
//! repair and the published manifest were read with plain `json.loads`, where NaN parses and a
//! format of true or 1.0 equals 1. U06 (§5 row 24): the merge took any Unicode digits for a
//! segment's number. The tests write to the remote with git directly.

use std::collections::BTreeSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use beadroll_core::json::{Format, write};
use beadroll_test_support::{Json, Machine, Sandbox, code, json};

/// `setUp`: machine `u05-a` has a tracker, prefix `ex`, whose one issue it has published.
fn set_up(sandbox: &Sandbox) -> Machine {
    let a = Machine::new(sandbox, "u05-a");
    a.bead(["init", "--prefix", "ex"]);
    a.bead(["create", "published"]);
    a.bead(["sync"]);
    a
}

/// `self.remote(*args, data=...)`: `git ARGS` on the sandbox's remote (`GIT_DIR`), as author and
/// committer `t` and with no other `GIT_*` variable, `data` on its standard input; what it printed,
/// stripped. A failure panics.
fn remote(sandbox: &Sandbox, args: &[&str], data: Option<&[u8]>) -> String {
    let inherited = std::env::vars_os().filter(|(key, _)| {
        let key = key.as_encoded_bytes();
        !key.starts_with(b"GIT_")
    });
    let mut command = Command::new("git");
    command
        .args(args)
        .env_clear()
        .envs(inherited)
        .env("GIT_DIR", sandbox.origin())
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
        .stdin(if data.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("git starts");
    if let (Some(mut input), Some(data)) = (child.stdin.take(), data) {
        // git reads all of it before it prints a word: no pipe fills while this writes
        input.write_all(data).expect("git takes its input");
    }
    let output = child.wait_with_output().expect("git's output");
    assert!(
        output.status.success(),
        "git {args:?} failed ({}): {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let printed = String::from_utf8(output.stdout).expect("git's output is UTF-8");
    printed.trim().to_owned()
}

/// `publish_manifest`: points the remote's journal ref at a commit holding the same journals and
/// `manifest`; the commit.
fn publish_manifest(sandbox: &Sandbox, manifest: &[u8]) -> String {
    let tip = remote(sandbox, &["rev-parse", "refs/beads/journal"], None);
    let events = remote(sandbox, &["rev-parse", &format!("{tip}:events")], None);
    let blob = remote(sandbox, &["hash-object", "-w", "--stdin"], Some(manifest));
    let entries = format!("040000 tree {events}\tevents\n100644 blob {blob}\tmanifest.json\n");
    let tree = remote(sandbox, &["mktree"], Some(entries.as_bytes()));
    let message = "a manifest no bead wrote";
    let commit = remote(
        sandbox,
        &["commit-tree", &tree, "-p", &tip, "-m", message],
        None,
    );
    remote(
        sandbox,
        &["update-ref", "refs/beads/journal", &commit, &tip],
        None,
    );
    commit
}

/// The entries of `dir` whose names `wanted` takes, in directory order, as `Path.glob` lists them.
fn entries(dir: &Path, wanted: impl Fn(&[u8]) -> bool) -> Vec<PathBuf> {
    let listing = std::fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("cannot list {}: {error}", dir.display()));
    listing
        .map(|entry| entry.expect("a directory entry").path())
        .filter(|path| wanted(path.file_name().expect("a name").as_encoded_bytes()))
        .collect()
}

/// U05: a published manifest whose format only equals 1 (`true` or `1.0`) is a format this bead
/// does not read (`FORMAT_UNSUPPORTED`), and one that holds NaN is unreadable (`MANIFEST_MISSING`);
/// either way the sync is refused and the remote's journal is left as it was.
#[test]
fn u05_a_manifest_that_only_equals_format_1_is_refused() {
    let sandbox = crate::sandbox();
    let a = set_up(&sandbox);
    let store = a.store();
    let workspace = store.file_name().expect("the store's name").to_str();
    let workspace = workspace.expect("a UTF-8 name");
    let cases = [
        ("true", "FORMAT_UNSUPPORTED"),
        ("1.0", "FORMAT_UNSUPPORTED"),
        (r#"1, "note": NaN"#, "MANIFEST_MISSING"),
    ];
    for (spelling, expected) in cases {
        let manifest = format!(r#"{{"format": {spelling}, "workspace": "{workspace}"}}"#) + "\n";
        let tip = publish_manifest(&sandbox, manifest.as_bytes());
        let synced = a.run(["sync", "--json"]);
        assert_eq!(code(&synced), expected, "format={spelling}");
        // nothing rewritten
        let now = remote(&sandbox, &["rev-parse", "refs/beads/journal"], None);
        assert_eq!(now, tip, "format={spelling}");
    }
}

/// U06 (`docs/RUST-PORT.md` §5 row 24): the merge took any Unicode digits for a segment's number,
/// so a published `events/<machine>/٠٠٠٠٠١.jsonl` was copied in and read as a journal.
#[test]
fn u06_only_segments_numbered_in_ascii_digits_are_merged() {
    let sandbox = crate::sandbox();
    set_up(&sandbox);
    let tip = remote(&sandbox, &["rev-parse", "refs/beads/journal"], None);
    let planted = json!({
        "v": 1,
        "id": "019a5555-5555-7000-8000-000000000001",
        "replica": "ffffffff",
        "deps": [],
        "actor": {"agent": "test"},
        "op": "create",
        "issue": "019a5555-5555-7000-8000-000000000001",
        "data": {"key": "ex-planted", "fields": {"title": "planted"}},
    });
    let line = write(&planted, Format::ASCII).expect("an event") + "\n";
    let blob = remote(
        &sandbox,
        &["hash-object", "-w", "--stdin"],
        Some(line.as_bytes()),
    );
    let name = "٠".repeat(5) + "١.jsonl";
    let entry = format!("100644 blob {blob}\t{name}\n");
    let folder = remote(&sandbox, &["mktree"], Some(entry.as_bytes()));
    let listing = remote(&sandbox, &["ls-tree", &format!("{tip}:events")], None);
    let entries = format!("{listing}\n040000 tree {folder}\tffffffff\n");
    let events = remote(&sandbox, &["mktree"], Some(entries.as_bytes()));
    let manifest = remote(
        &sandbox,
        &["rev-parse", &format!("{tip}:manifest.json")],
        None,
    );
    let entries = format!("040000 tree {events}\tevents\n100644 blob {manifest}\tmanifest.json\n");
    let tree = remote(&sandbox, &["mktree"], Some(entries.as_bytes()));
    let message = "a segment no bead wrote";
    let commit = remote(
        &sandbox,
        &["commit-tree", &tree, "-p", &tip, "-m", message],
        None,
    );
    remote(
        &sandbox,
        &["update-ref", "refs/beads/journal", &commit, &tip],
        None,
    );
    let b = Machine::new(&sandbox, "u06-b");
    b.bead(["sync"]);
    let listed = b.bead(["list", "--all", "--limit", "0"]).unwrap();
    let issues = listed["issues"].as_array().expect("a list of issues");
    let titles: BTreeSet<&str> = issues
        .iter()
        .map(|issue| issue["title"].as_str().expect("a title"))
        .collect();
    assert_eq!(titles, BTreeSet::from(["published"]));
    let copied = b.store().join("events").join("ffffffff").join(&name);
    assert!(!copied.exists(), "{} was merged", copied.display());
}

/// U05: a torn tail that holds NaN is not a complete event: the next write cuts it off and keeps it
/// aside, byte for byte, instead of completing it, and the tracker reports no damage.
#[test]
fn u05_a_torn_tail_holding_nan_is_cut_and_kept_aside() {
    let sandbox = crate::sandbox();
    let a = set_up(&sandbox);
    let info = a.bead(["info"]).unwrap();
    let own = info["machine"].as_str().expect("the machine's id");
    let folder = a.store().join("events").join(own);
    let mut segments = entries(&folder, |name| name.ends_with(b".jsonl"));
    segments.sort();
    let segment = segments.pop().expect("a segment");
    let (before, fragment) = (std::fs::read(&segment).unwrap(), br#"{"v":1,"note":NaN}"#);
    std::fs::write(&segment, [before.as_slice(), fragment].concat()).unwrap();
    a.bead(["create", "after the fragment"]);
    assert!(std::fs::read(&segment).unwrap().starts_with(&before));
    let now = std::fs::read(&segment).unwrap();
    assert!(!now.windows(3).any(|bytes| bytes == b"NaN"), "{now:?}");
    let name = segment.file_name().expect("a name").as_encoded_bytes();
    let torn = [name, b".torn-"].concat();
    let aside = entries(&folder, |entry| entry.starts_with(&torn));
    let kept: Vec<Vec<u8>> = aside
        .iter()
        .map(|path| std::fs::read(path).unwrap())
        .collect();
    assert_eq!(kept, [fragment.to_vec()]);
    let integrity = a.bead(["info"]).unwrap()["integrity"].clone();
    assert_eq!(integrity, Json::Array(Vec::new()));
}
