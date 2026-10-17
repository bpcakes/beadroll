//! A prune of a machine's own sync.git between a fetch and the reads of what it brought, made
//! deterministic: `fetch.fetched` holds the sync there while the test takes one object of the
//! fetched tip from sync.git, as `git gc --prune=now` racing the fetch did on macOS CI
//! (roll-dih8.30). An object of the tip missing or unreadable here is local damage, which the
//! command repairs by making sync.git again, whichever read finds it: the manifest's, the listing's
//! or a blob's. A tip whose objects are all here keeps the error that is the remote's:
//! MANIFEST_MISSING for a tree without a manifest blob, GIT_ERROR for a gitlink where a segment
//! goes.

use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::{Command, Stdio};

use beadroll_test_support::{Json, Machine, Run, Sandbox, code, parse_json};

use crate::hardening::gate::{Gate, beside};

/// The remote's tip, and in it, as revisions, its tree and its manifest's blob; `{b}` stands for
/// machine b's folder under `events/`.
const TIP: &str = "refs/beads/journal";
const TREE: &str = "refs/beads/journal^{tree}";
const MANIFEST: &str = "refs/beads/journal:manifest.json";
const B_FOLDER: &str = "refs/beads/journal:events/{b}";
const B_SEGMENT: &str = "refs/beads/journal:events/{b}/000001.jsonl";

/// What the test does to the object in a's sync.git.
#[derive(Clone, Copy)]
enum Harm {
    /// Removes it, as a prune does.
    Remove,
    /// Leaves it empty: there, and unreadable.
    Empty,
}

/// `git --git-dir GIT_DIR ARGS`, given `input`: its output, trimmed.
fn plumbing(git_dir: &Path, args: &[&str], input: Option<&str>) -> String {
    let mut child = Command::new("git")
        .arg("--git-dir")
        .arg(git_dir)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("git starts");
    let mut stdin = child.stdin.take().expect("git's input");
    if let Some(input) = input {
        std::io::Write::write_all(&mut stdin, input.as_bytes()).expect("git reads");
    }
    drop(stdin);
    let output = child.wait_with_output().expect("git ends");
    assert!(output.status.success(), "{args:?}");
    String::from_utf8(output.stdout)
        .expect("git's output is UTF-8")
        .trim()
        .to_owned()
}

/// The names under `events/` at `revision` in the bare repository `git_dir`.
fn folders(git_dir: &Path, revision: &str) -> Vec<String> {
    let tree = format!("{revision}:events");
    let listed = plumbing(git_dir, &["ls-tree", "--name-only", &tree], None);
    listed.lines().map(str::to_owned).collect()
}

/// Harms `object` in the bare repository `git_dir`: every pack is unpacked into loose objects
/// first (a fetch into sync.git leaves what it brought packed), from a copy set aside in `aside`,
/// so that the one object can be harmed alone.
fn harm(git_dir: &Path, aside: &Path, object: &str, harm: Harm) {
    std::fs::create_dir(aside).expect("a directory for the packs set aside");
    let packs = git_dir.join("objects").join("pack");
    let mut moved = Vec::new();
    for entry in std::fs::read_dir(&packs).expect("sync.git's packs") {
        let path = entry.expect("a pack's entry").path();
        let target = aside.join(path.file_name().expect("a file name"));
        std::fs::rename(&path, &target).expect("a pack set aside");
        if target.extension().is_some_and(|e| e == "pack") {
            moved.push(target);
        }
    }
    for pack in moved {
        let input = std::fs::File::open(&pack).expect("a pack set aside");
        let unpacked = Command::new("git")
            .arg("--git-dir")
            .arg(git_dir)
            .args(["unpack-objects", "-q"])
            .stdin(Stdio::from(input))
            .output()
            .expect("git starts");
        assert!(
            unpacked.status.success(),
            "{}",
            String::from_utf8_lossy(&unpacked.stderr)
        );
    }
    let loose = git_dir
        .join("objects")
        .join(&object[..2])
        .join(&object[2..]);
    match harm {
        Harm::Remove => std::fs::remove_file(&loose).expect("the object, loose"),
        Harm::Empty => {
            std::fs::set_permissions(&loose, std::fs::Permissions::from_mode(0o644))
                .expect("the object, loose");
            std::fs::write(&loose, b"").expect("the object emptied");
        }
    }
}

/// Machine a (this build: it stops at `fetch.fetched`) publishes issue x and machine b then
/// publishes "from b"; a syncs, held once its fetch has brought b's tip while the object
/// `revision` names there is harmed in a's sync.git. That sync's run, and a.
fn sync_harming(sandbox: &Sandbox, revision: &str, how: Harm) -> (Run, Machine) {
    let a = crate::rust_only(sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    a.bead(["create", "x"]);
    a.bead(["sync"]);
    let b = Machine::new(sandbox, "b");
    b.bead(["sync"]);
    b.bead(["create", "from b"]);
    b.bead(["sync"]);
    let (origin, sync_git) = (sandbox.origin(), a.store().join("sync.git"));
    let theirs = folders(&sync_git, "refs/beads/remote");
    let b_folder = folders(origin, TIP)
        .into_iter()
        .find(|folder| !theirs.contains(folder))
        .expect("b's folder, which a has not fetched");
    let object = plumbing(
        origin,
        &["rev-parse", &revision.replace("{b}", &b_folder)],
        None,
    );
    let fetched = Gate::new(sandbox, "fetched");
    let failpoints = format!("fetch.fetched={}", fetched.action());
    let (run, harmed) = beside(
        || {
            let sync = a.cmd(["sync", "--json"]);
            sync.env("BEAD_FAILPOINTS", &failpoints).run()
        },
        |finished| {
            fetched.meet(finished, || {
                harm(&sync_git, &sandbox.root().join("aside"), &object, how);
            })
        },
    );
    assert!(harmed.is_some(), "the sync never fetched: {}", run.stderr);
    (run, a)
}

/// The sync went through after making sync.git again, and brought b's issue; the next makes none.
fn assert_rebuilt_and_synced(run: &Run, a: &Machine) {
    assert_eq!(run.returncode(), 0, "{}{}", run.stdout, run.stderr);
    let result = parse_json(&run.stdout).expect("a sync's record");
    let rebuilt = result.get("sync_repo_rebuilt").and_then(Json::as_str);
    let aside = format!("{}.damaged-", a.store().join("sync.git").display());
    assert!(
        rebuilt.unwrap_or_default().starts_with(&aside),
        "{result:?}"
    );
    let listed = a.bead(["list"]).expect("a list");
    let mut titles: Vec<&str> = listed["issues"]
        .as_array()
        .expect("a list of issues")
        .iter()
        .map(|issue| issue["title"].as_str().expect("a title"))
        .collect();
    titles.sort_unstable();
    assert_eq!(titles, ["from b", "x"]);
    let again = parse_json(&a.run(["sync", "--json"]).stdout).expect("a sync's record");
    assert!(again.get("sync_repo_rebuilt").is_none(), "{again:?}");
}

/// The fetched tip's tree gone from sync.git between the fetch and the manifest's read: local
/// damage, not a remote without a manifest (MANIFEST_MISSING before roll-dih8.30).
#[test]
fn a_fetched_tree_pruned_before_the_manifest_is_read_is_local_damage() {
    let sandbox = crate::sandbox();
    let (run, a) = sync_harming(&sandbox, TREE, Harm::Remove);
    assert_rebuilt_and_synced(&run, &a);
}

/// The fetched tip's commit gone from sync.git after the fetch named it: local damage.
#[test]
fn a_fetched_commit_pruned_before_it_is_read_is_local_damage() {
    let sandbox = crate::sandbox();
    let (run, a) = sync_harming(&sandbox, TIP, Harm::Remove);
    assert_rebuilt_and_synced(&run, &a);
}

/// The manifest's blob gone from sync.git while the fetched tree, whole, lists it: local damage.
#[test]
fn a_manifest_blob_the_fetched_tree_lists_but_sync_git_lacks_is_local_damage() {
    let sandbox = crate::sandbox();
    let (run, a) = sync_harming(&sandbox, MANIFEST, Harm::Remove);
    assert_rebuilt_and_synced(&run, &a);
}

/// The manifest's blob there but unreadable (an empty object file): local damage too.
#[test]
fn a_manifest_blob_git_cannot_read_is_local_damage() {
    let sandbox = crate::sandbox();
    let (run, a) = sync_harming(&sandbox, MANIFEST, Harm::Empty);
    assert_rebuilt_and_synced(&run, &a);
}

/// A subtree of the fetched tip gone, which the merge's listing reads after the manifest: local
/// damage, where it was GIT_ERROR, and every later sync's too, the tip being one sync.git's ref
/// already named, so that no fetch brought it again.
#[test]
fn a_fetched_subtree_pruned_before_the_merge_lists_it_is_local_damage() {
    let sandbox = crate::sandbox();
    let (run, a) = sync_harming(&sandbox, B_FOLDER, Harm::Remove);
    assert_rebuilt_and_synced(&run, &a);
}

/// A segment's blob of the fetched tip gone, which the merge reads: local damage, where it was
/// GIT_ERROR on every sync after.
#[test]
fn a_fetched_segment_pruned_before_the_merge_reads_it_is_local_damage() {
    let sandbox = crate::sandbox();
    let (run, a) = sync_harming(&sandbox, B_SEGMENT, Harm::Remove);
    assert_rebuilt_and_synced(&run, &a);
}

/// Machine a (this build) publishes issue x; the remote's tip is then replaced by a commit of the
/// tree `listing` makes there, on top of it. a's sync of it.
fn sync_replaced_tip(sandbox: &Sandbox, listing: impl Fn(&Path) -> String) -> (Run, Machine) {
    let a = crate::rust_only(sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    a.bead(["create", "x"]);
    a.bead(["sync"]);
    let origin = sandbox.origin();
    let tree = plumbing(origin, &["mktree"], Some(&listing(origin)));
    let tip = plumbing(origin, &["rev-parse", TIP], None);
    let commit = plumbing(origin, &["commit-tree", &tree, "-p", &tip, "-m", "x"], None);
    plumbing(origin, &["update-ref", TIP, &commit, &tip], None);
    (a.run(["sync", "--json"]), a)
}

/// sync.git was never set aside.
fn assert_kept(a: &Machine) {
    let info = a.bead(["info"]).expect("an info record");
    let damaged = info.get("damaged_sync_repos").and_then(Json::as_array);
    assert_eq!(damaged.map_or(0, <[Json]>::len), 0, "{info:?}");
}

/// A tip whose tree is whole but holds no manifest.json blob, none at all or a tree by that name,
/// is the remote's: MANIFEST_MISSING, sync.git kept.
#[test]
fn a_whole_tree_without_a_manifest_blob_is_still_manifest_missing() {
    for named in [false, true] {
        let sandbox = crate::sandbox();
        let (run, a) = sync_replaced_tip(&sandbox, |origin| {
            let events = plumbing(origin, &["rev-parse", &format!("{TIP}:events")], None);
            let mut listing = format!("040000 tree {events}\tevents\n");
            if named {
                let manifest = plumbing(origin, &["rev-parse", MANIFEST], None);
                let inside = format!("100644 blob {manifest}\tmanifest.json\n");
                let inner = plumbing(origin, &["mktree"], Some(&inside));
                listing.push_str(&format!("040000 tree {inner}\tmanifest.json\n"));
            }
            listing
        });
        assert_eq!(code(&run), "MANIFEST_MISSING", "{}", run.stderr);
        assert_kept(&a);
    }
}

/// A gitlink where a segment goes is a commit no fetch brings, not an object sync.git lost: the
/// merge's failed read of it stays GIT_ERROR, sync.git kept.
#[test]
fn a_gitlink_where_a_segment_goes_is_not_local_damage() {
    let sandbox = crate::sandbox();
    let (run, a) = sync_replaced_tip(&sandbox, |origin| {
        let manifest = plumbing(origin, &["rev-parse", MANIFEST], None);
        let code_tip = plumbing(origin, &["rev-parse", "main"], None);
        let link = format!("160000 commit {code_tip}\t000001.jsonl\n");
        let folder = plumbing(origin, &["mktree"], Some(&link));
        let events = plumbing(
            origin,
            &["mktree"],
            Some(&format!("040000 tree {folder}\tf00dfeed\n")),
        );
        format!("040000 tree {events}\tevents\n100644 blob {manifest}\tmanifest.json\n")
    });
    assert_eq!(code(&run), "GIT_ERROR", "{}", run.stderr);
    assert_kept(&a);
}
