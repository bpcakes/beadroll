//! A prune of a machine's own sync.git between a fetch and the reads of what it brought, made
//! deterministic: `fetch.fetched` holds the sync there while the test takes one object of the
//! fetched tip from sync.git, as `git gc --prune=now` racing the fetch likely did on macOS CI
//! (roll-dih8.30), or damages it as a disk might. An object of the tip missing here, or a blob of
//! it here but unreadable, is local damage, which the command repairs by making sync.git again,
//! whichever read finds it: the manifest's, the listing's or a blob's, and so is a tree of it
//! malformed here, which no fetch lets in. A tip whose objects are all here and readable keeps the
//! error that is the remote's: MANIFEST_MISSING for a tree without a manifest blob, GIT_ERROR for
//! a gitlink where a segment goes.

use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::{Command, Stdio};

use beadroll_test_support::{Json, Machine, Run, Sandbox, code, parse_json};

use crate::hardening::gate::{Gate, beside};

mod mismatched_trees;

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
    /// Changes the byte in the middle of its file: there, and unreadable, in no words git uses
    /// for damage ("unable to stream" for a blob).
    Flip,
    /// Cuts the last byte off its file: the same.
    Cut,
    /// Replaces a tree's file by a whole one of a malformed tree, which every read and walk of it
    /// fails on in no words of damage ("too-short tree object"), and which no fetch lets in.
    Malformed,
    /// Replaces a tree's file with a different, well-formed tree, empty or holding one blob.
    OtherTree { empty: bool },
    /// Keeps the root's manifest, so reading it succeeds before the listing finds the mismatch.
    OnlyManifest,
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
        Harm::Malformed | Harm::OtherTree { .. } | Harm::OnlyManifest => {
            let made = match harm {
                Harm::OnlyManifest => {
                    let manifest = plumbing(
                        git_dir,
                        &["rev-parse", &format!("{object}:manifest.json")],
                        None,
                    );
                    plumbing(
                        git_dir,
                        &["mktree"],
                        Some(&format!("100644 blob {manifest}\tmanifest.json\n")),
                    )
                }
                Harm::OtherTree { empty: true } => plumbing(git_dir, &["mktree"], Some("")),
                Harm::OtherTree { empty: false } => {
                    let blob = plumbing(git_dir, &["hash-object", "-w", "--stdin"], Some("x"));
                    plumbing(
                        git_dir,
                        &["mktree"],
                        Some(&format!("100644 blob {blob}\tother\n")),
                    )
                }
                _ => {
                    let entry = "100644 x\0\u{1}\u{1}\u{1}\u{1}\u{1}\u{1}\u{1}";
                    let write = ["hash-object", "-t", "tree", "--literally", "-w", "--stdin"];
                    plumbing(git_dir, &write, Some(entry))
                }
            };
            let made = git_dir.join("objects").join(&made[..2]).join(&made[2..]);
            std::fs::set_permissions(&loose, std::fs::Permissions::from_mode(0o644))
                .expect("the object, loose");
            std::fs::copy(made, &loose).expect("the object replaced");
        }
        Harm::Empty | Harm::Flip | Harm::Cut => {
            let mut bytes = std::fs::read(&loose).expect("the object, loose");
            match harm {
                Harm::Flip => {
                    let middle = bytes.len() / 2;
                    bytes[middle] ^= 0xff;
                }
                Harm::Cut => drop(bytes.pop()),
                _ => bytes.clear(),
            }
            std::fs::set_permissions(&loose, std::fs::Permissions::from_mode(0o644))
                .expect("the object, loose");
            std::fs::write(&loose, bytes).expect("the object harmed");
        }
    }
}

/// a's `sync --json`, held once its fetch has brought the remote's tip while `harmed` runs. Its
/// run.
fn sync_held(sandbox: &Sandbox, a: &Machine, harmed: impl FnOnce()) -> Run {
    let fetched = Gate::new(sandbox, "fetched");
    let failpoints = format!("fetch.fetched={}", fetched.action());
    let (run, met) = beside(
        || {
            let sync = a.cmd(["sync", "--json"]);
            sync.env("BEAD_FAILPOINTS", &failpoints).run()
        },
        |finished| fetched.meet(finished, harmed),
    );
    assert!(met.is_some(), "the sync never fetched: {}", run.stderr);
    run
}

/// Machine a (this build: it stops at `fetch.fetched`) publishes issue x and machine b then
/// publishes "from b"; a syncs, held once its fetch has brought b's tip while the object
/// `revision` names there is harmed in a's sync.git. That sync's run, and a.
fn sync_harming(sandbox: &Sandbox, revision: &str, how: Harm) -> (Run, Machine) {
    sync_harming_with(sandbox, revision, how, |_, _| {})
}

/// [`sync_harming`], after `before_fetch` has prepared the remote's tip and machine a's run.
fn sync_harming_with(
    sandbox: &Sandbox,
    revision: &str,
    how: Harm,
    before_fetch: impl FnOnce(&Path, &mut Machine),
) -> (Run, Machine) {
    let mut a = crate::rust_only(sandbox, "a");
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
    before_fetch(origin, &mut a);
    let run = sync_held(sandbox, &a, || {
        harm(&sync_git, &sandbox.root().join("aside"), &object, how);
    });
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

/// The manifest's blob there but unreadable, its object file emptied, corrupt or cut short: local
/// damage too, where the last two, which git reports in no words of damage, were MANIFEST_MISSING
/// on every sync.
#[test]
fn a_manifest_blob_git_cannot_read_is_local_damage() {
    for how in [Harm::Empty, Harm::Flip, Harm::Cut] {
        let sandbox = crate::sandbox();
        let (run, a) = sync_harming(&sandbox, MANIFEST, how);
        assert_rebuilt_and_synced(&run, &a);
    }
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

/// A segment's blob of the fetched tip cut short or corrupt, which the merge reads: local damage,
/// where it was GIT_ERROR on every sync after.
#[test]
fn a_fetched_segment_git_cannot_read_is_local_damage() {
    for how in [Harm::Cut, Harm::Flip] {
        let sandbox = crate::sandbox();
        let (run, a) = sync_harming(&sandbox, B_SEGMENT, how);
        assert_rebuilt_and_synced(&run, &a);
    }
}

/// A tree of the fetched tip malformed in a's sync.git, b's folder or the root: local damage, found
/// by the walk over the tip, where it was GIT_ERROR or MANIFEST_MISSING on every sync after.
#[test]
fn a_fetched_tree_git_cannot_walk_is_local_damage() {
    for revision in [B_FOLDER, TREE] {
        let sandbox = crate::sandbox();
        let (run, a) = sync_harming(&sandbox, revision, Harm::Malformed);
        assert_rebuilt_and_synced(&run, &a);
    }
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

/// What the remote's tree holds by the name manifest.json, in place of a blob.
#[derive(Clone, Copy)]
enum NoManifest {
    /// Nothing.
    Absent,
    /// A tree holding the manifest's blob.
    Tree,
    /// A gitlink to the tip it replaced, a commit a's sync.git holds.
    Gitlink,
}

/// The remote's tree of the tip that replaced its own, holding `events/` and, by the name
/// manifest.json, what `instead` says.
fn without_a_manifest(origin: &Path, instead: NoManifest) -> String {
    let events = plumbing(origin, &["rev-parse", &format!("{TIP}:events")], None);
    let mut listing = format!("040000 tree {events}\tevents\n");
    match instead {
        NoManifest::Absent => {}
        NoManifest::Tree => {
            let manifest = plumbing(origin, &["rev-parse", MANIFEST], None);
            let inside = format!("100644 blob {manifest}\tmanifest.json\n");
            let inner = plumbing(origin, &["mktree"], Some(&inside));
            listing.push_str(&format!("040000 tree {inner}\tmanifest.json\n"));
        }
        NoManifest::Gitlink => {
            let tip = plumbing(origin, &["rev-parse", TIP], None);
            listing.push_str(&format!("160000 commit {tip}\tmanifest.json\n"));
        }
    }
    listing
}

/// A tip whose tree is whole but holds no manifest.json blob, none at all, a tree by that name or
/// a gitlink to a commit sync.git holds, is the remote's: MANIFEST_MISSING, sync.git kept.
#[test]
fn a_whole_tree_without_a_manifest_blob_is_still_manifest_missing() {
    for instead in [NoManifest::Absent, NoManifest::Tree, NoManifest::Gitlink] {
        let sandbox = crate::sandbox();
        let (run, a) = sync_replaced_tip(&sandbox, |origin| without_a_manifest(origin, instead));
        assert_eq!(code(&run), "MANIFEST_MISSING", "{}", run.stderr);
        assert_kept(&a);
    }
}

/// A tip without a manifest blob read while a tree of it is corrupt in a's sync.git: git's own
/// words for the tree, which only the walk over the tip reads, tell the damage, so sync.git is
/// made again, once, and the remote's MANIFEST_MISSING follows.
#[test]
fn a_tree_corrupt_beside_a_missing_manifest_is_local_damage_first() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    a.bead(["create", "x"]);
    a.bead(["sync"]);
    let origin = sandbox.origin();
    let events = format!("{TIP}:events");
    let a_folder = format!("{events}/{}", folders(origin, TIP)[0]);
    let a_folder = plumbing(origin, &["rev-parse", &a_folder], None);
    let tree = plumbing(
        origin,
        &["mktree"],
        Some(&without_a_manifest(origin, NoManifest::Absent)),
    );
    let tip = plumbing(origin, &["rev-parse", TIP], None);
    let commit = plumbing(origin, &["commit-tree", &tree, "-p", &tip, "-m", "x"], None);
    plumbing(origin, &["update-ref", TIP, &commit, &tip], None);
    let sync_git = a.store().join("sync.git");
    let run = sync_held(&sandbox, &a, || {
        harm(
            &sync_git,
            &sandbox.root().join("aside"),
            &a_folder,
            Harm::Flip,
        );
    });
    assert_eq!(code(&run), "MANIFEST_MISSING", "{}", run.stderr);
    let info = a.bead(["info"]).expect("an info record");
    let damaged = info.get("damaged_sync_repos").and_then(Json::as_array);
    assert_eq!(damaged.map_or(0, <[Json]>::len), 1, "{info:?}");
}

/// A gitlink where a segment goes is a commit no fetch brings, or one sync.git holds for another
/// reason, not an object sync.git lost or damaged: the merge's failed read of it stays GIT_ERROR,
/// sync.git kept.
#[test]
fn a_gitlink_where_a_segment_goes_is_not_local_damage() {
    for linked in ["main", TIP] {
        let sandbox = crate::sandbox();
        let (run, a) = sync_replaced_tip(&sandbox, |origin| gitlink_for_a_segment(origin, linked));
        assert_eq!(code(&run), "GIT_ERROR", "{}", run.stderr);
        assert_kept(&a);
    }
}

/// The remote's tree of the tip that replaced its own: its manifest, and one folder whose segment
/// is a gitlink to the commit `linked` names in the remote, `main` (not fetched) or the journal's
/// tip it replaced (in a's sync.git).
fn gitlink_for_a_segment(origin: &Path, linked: &str) -> String {
    let manifest = plumbing(origin, &["rev-parse", MANIFEST], None);
    let linked = plumbing(origin, &["rev-parse", linked], None);
    let link = format!("160000 commit {linked}\t000001.jsonl\n");
    let folder = plumbing(origin, &["mktree"], Some(&link));
    let events = plumbing(
        origin,
        &["mktree"],
        Some(&format!("040000 tree {folder}\tf00dfeed\n")),
    );
    format!("040000 tree {events}\tevents\n100644 blob {manifest}\tmanifest.json\n")
}
