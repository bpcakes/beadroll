//! What a publish hashes (roll-lqjj.1): the journal files the store's memo cannot vouch for, each
//! once, and nothing else, while the tree it publishes is the one hashing every file makes; a memo
//! that is missing, stale or unreadable is never trusted.

use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};

use beadroll_core::json::{Object, Value};
use beadroll_store::{
    Endpoint, Workspace, claim_files, memo_path, relative, replica_id, segment_files,
};
use beadroll_sync::{Error, Link, PushOutcome, locked, writing};
use beadroll_test_support::{Machine, Sandbox, git};

use crate::support::{actor, claim_command, sandbox, workspace};

mod trees;

/// What one round did: the files its link hashed, the paths it found forked, and the tree the
/// remote holds after it.
struct Round {
    hashed: Vec<String>,
    forks: Vec<String>,
    tree: String,
    reused_trees: Vec<String>,
}

/// A round of a sync through a link of its own, as a command makes one: a fetch, then a merge and
/// a publish under the store lock, which goes through.
fn round(ws: &Workspace, origin: &Path) -> Round {
    let mut link = Link::new(ws, &Endpoint::at(origin)).expect("a link");
    let tip = link.fetch().expect("a fetch");
    let outcome = locked(ws, || -> Result<PushOutcome, Error> {
        link.merge(tip.as_deref())?;
        link.publish(tip.as_deref(), "sync", &[], false)
    })
    .expect("a round");
    assert_eq!(outcome, PushOutcome::Ok);
    let commit = link.last_commit.clone().expect("the remote's head");
    let origin = origin.to_str().expect("a path in UTF-8");
    let spec = format!("{commit}^{{tree}}");
    let tree = git(["--git-dir", origin, "rev-parse", spec.as_str()], None);
    Round {
        hashed: link.hashed.clone(),
        forks: link.forks.clone(),
        tree: tree.trim().to_owned(),
        reused_trees: link.reused_trees.clone(),
    }
}

/// Every journal file of the store, by its path there, in the order a link hashes them.
fn journal_files(ws: &Workspace) -> Vec<String> {
    let mut files = segment_files(ws).expect("the segments");
    files.extend(claim_files(ws).expect("the attempt files").iter().cloned());
    files.iter().map(|file| relative(ws, file)).collect()
}

/// The round after `with_memo` made with no memo at all: every file is hashed, once, and the tree
/// is the one `with_memo` published.
fn same_tree_without_memo(ws: &Workspace, origin: &Path, with_memo: &Round) {
    std::fs::remove_file(memo_path(ws)).expect("a memo to remove");
    let full = round(ws, origin);
    assert_eq!(full.hashed, journal_files(ws), "every file, once");
    assert_eq!(
        full.tree, with_memo.tree,
        "the tree hashing every file makes"
    );
}

/// A comment on `key`, written as `machine`'s local write on `ws`: appended to its journal as the
/// settings of `ws` rotate it, and not published.
fn comment(ws: &Workspace, machine: &Machine, key: &str, text: &str) {
    writing(ws, &actor(machine), |tx| -> Result<(), Error> {
        let issue = tx
            .state()?
            .resolve(key)
            .map(|issue| issue.uid().to_string());
        let mut data = Object::new();
        data.insert("text".into(), Value::from(text));
        tx.emit("comment", issue.as_deref(), data, None)?;
        Ok(())
    })
    .expect("a comment");
}

/// Machine a's tracker with issue x, published, and machine b's issue beside it: a's store holds
/// a journal of its own and b's. a, its store opened here, x's key, and b.
fn set_up(sandbox: &Sandbox) -> (Machine, Workspace, String, Machine) {
    let a = Machine::new(sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let created = a.bead(["create", "x"]).expect("a create");
    let key = created["id"].as_str().expect("a key").to_owned();
    let b = Machine::new(sandbox, "b");
    b.bead(["sync"]);
    b.bead(["create", "from b"]);
    let ws = workspace(&a, sandbox.origin(), &[]);
    // a first round brings b's journal in
    round(&ws, sandbox.origin());
    (a, ws, key, b)
}

/// This machine's segment `n`, by its path in the store.
fn own_segment(ws: &Workspace, n: u32) -> String {
    let me = replica_id(&ws.settings, false)
        .expect("an identity")
        .expect("this machine's identity");
    format!("events/{me}/{n:06}.jsonl")
}

/// A publish hashes the active segment once, not in its merge and again for its tree; after a
/// rotation, only the new segment; a full segment whose torn tail was repaired, since its length
/// changed; and a new claim's attempt file, once. Another machine's segment that a merge extends is
/// the published blob, and is not hashed. The tree is each time the one hashing every file makes.
#[test]
fn a_publish_hashes_only_what_changed_across_a_rotation_a_repaired_tail_and_a_claim() {
    let sandbox = sandbox();
    let origin = sandbox.origin();
    let (a, ws, key, b) = set_up(&sandbox);
    let rotating = workspace(&a, origin, &[("BEAD_ROTATE_BYTES", "1")]);
    let (first, second) = (own_segment(&ws, 1), own_segment(&ws, 2));
    assert_eq!(round(&ws, origin).hashed, Vec::<String>::new());

    b.bead(["create", "more from b"]);
    let extended = round(&ws, origin);
    assert_eq!(extended.hashed, Vec::<String>::new(), "b's segment came in");
    same_tree_without_memo(&ws, origin, &extended);

    comment(&ws, &a, &key, "one");
    let appended = round(&ws, origin);
    assert_eq!(appended.hashed, [first.as_str()]);
    same_tree_without_memo(&ws, origin, &appended);

    comment(&rotating, &a, &key, "two");
    assert!(ws.dir.join(&second).exists(), "the journal rotated");
    let rotated = round(&ws, origin);
    assert_eq!(
        rotated.hashed,
        [second.as_str()],
        "the full segment is remembered"
    );
    same_tree_without_memo(&ws, origin, &rotated);
    comment(&ws, &a, &key, "three");
    assert_eq!(round(&ws, origin).hashed, [second.as_str()]);

    let full = ws.dir.join(&first);
    let mut torn = OpenOptions::new()
        .append(true)
        .open(&full)
        .expect("the full segment");
    torn.write_all(b"{\"v\": 1, \"id").expect("a torn line");
    drop(torn);
    let repaired = round(&ws, origin);
    assert_eq!(repaired.hashed, [first.as_str()]);
    same_tree_without_memo(&ws, origin, &repaired);

    claim_command(&ws, &actor(&a), &key).expect("a claim");
    let attempts: Vec<String> = claim_files(&ws)
        .expect("the attempt files")
        .iter()
        .map(|file| relative(&ws, file))
        .collect();
    assert_eq!(attempts.len(), 1, "{attempts:?}");
    let claimed = round(&ws, origin);
    assert_eq!(claimed.hashed, attempts);
    same_tree_without_memo(&ws, origin, &claimed);
    assert_eq!(round(&ws, origin).hashed, Vec::<String>::new());
}

/// A memo that is missing or cannot be read is none: every file is hashed, and a publish goes
/// through as without one, also when the memo cannot be written. One that names blobs the
/// published tree does not hold at those paths is not trusted for them, and one whose file was
/// rewritten in place, its length kept, not for that file: it is hashed and merged as before, the
/// published copy replacing the damaged one.
#[test]
fn a_memo_that_is_missing_stale_or_unreadable_is_not_trusted() {
    let sandbox = sandbox();
    let origin = sandbox.origin();
    let (_a, ws, _, _b) = set_up(&sandbox);
    let quiet = round(&ws, origin);
    assert_eq!(quiet.hashed, Vec::<String>::new());
    let all = journal_files(&ws);
    let memo = memo_path(&ws);

    std::fs::remove_file(&memo).expect("a memo");
    let missing = round(&ws, origin);
    assert_eq!(
        (missing.hashed, missing.tree),
        (all.clone(), quiet.tree.clone())
    );

    std::fs::write(&memo, b"bead journal memo 1\nnot a record").expect("a memo");
    assert_eq!(round(&ws, origin).hashed, all, "an unreadable memo");
    assert_eq!(
        round(&ws, origin).hashed,
        Vec::<String>::new(),
        "written again"
    );

    std::fs::remove_file(&memo).expect("a memo");
    std::fs::create_dir(&memo).expect("a directory where the memo goes");
    for _ in 0..2 {
        let blocked = round(&ws, origin);
        assert_eq!(
            (blocked.hashed, blocked.tree),
            (all.clone(), quiet.tree.clone())
        );
    }
    std::fs::remove_dir(&memo).expect("the directory");
    round(&ws, origin);

    // two remembered blobs exchanged: neither is what the published tree holds at its path
    let text = String::from_utf8(std::fs::read(&memo).expect("a memo")).expect("text");
    let blobs: Vec<&str> = text
        .lines()
        .filter_map(|line| line.rsplit(' ').next())
        .filter(|blob| blob.len() == 40)
        .collect();
    let (one, two) = (blobs[0].to_owned(), blobs[1].to_owned());
    assert_ne!(one, two);
    let swapped = text
        .replace(&one, "-swap-")
        .replace(&two, &one)
        .replace("-swap-", &two);
    std::fs::write(&memo, swapped).expect("a memo");
    let exchanged = round(&ws, origin);
    assert_eq!(exchanged.hashed, all[..2].to_vec());
    assert_eq!(exchanged.tree, quiet.tree);

    // b's segment, damaged here in place, its length kept
    let foreign = all
        .iter()
        .find(|path| !path.starts_with(&format!("events/{}", own_machine(&ws))))
        .expect("b's segment")
        .clone();
    let file = ws.dir.join(&foreign);
    let mut bytes = std::fs::read(&file).expect("b's segment");
    let at = bytes.iter().position(|b| *b == b'{').expect("a record");
    bytes[at] = b'[';
    std::fs::write(&file, &bytes).expect("damage");
    let damaged = round(&ws, origin);
    assert_eq!(damaged.hashed, [foreign.as_str()]);
    assert_eq!(damaged.forks, [foreign.as_str()]);
    assert_eq!(damaged.tree, quiet.tree, "the published copy is back");
}

/// This machine's identity.
fn own_machine(ws: &Workspace) -> String {
    replica_id(&ws.settings, false)
        .expect("an identity")
        .expect("this machine's identity")
}

/// A blob the memo remembers that `sync.git` no longer holds (a merge hashed the file, nothing
/// published it, and a gc dropped it) is hashed again rather than named in a tree: no ref reaches
/// it, so the published tree is not there to vouch for it.
#[test]
fn a_remembered_blob_sync_git_lost_is_hashed_again() {
    let sandbox = sandbox();
    let origin = sandbox.origin();
    let (a, ws, key, _b) = set_up(&sandbox);
    comment(&ws, &a, &key, "unpublished");
    let first = own_segment(&ws, 1);
    let mut link = Link::new(&ws, &Endpoint::at(origin)).expect("a link");
    let tip = link.fetch().expect("a fetch");
    locked(&ws, || link.merge(tip.as_deref())).expect("a merge");
    assert_eq!(link.hashed, [first.as_str()]);
    drop(link);
    let sync_git = ws.dir.join("sync.git");
    let blob = git(
        ["hash-object", ws.dir.join(&first).to_str().expect("UTF-8")],
        None,
    );
    let blob = blob.trim();
    let has = |blob: &str| {
        Command::new("git")
            .arg("--git-dir")
            .arg(&sync_git)
            .args(["cat-file", "-e", blob])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("git runs")
            .success()
    };
    assert!(has(blob), "the merge wrote it");
    let sync_dir = sync_git.to_str().expect("UTF-8");
    git(
        ["--git-dir", sync_dir, "gc", "--prune=now", "--quiet"],
        None,
    );
    assert!(!has(blob), "the gc dropped it");
    let after = round(&ws, origin);
    assert_eq!(after.hashed, [first.as_str()]);
    assert!(has(blob));
}

/// A blob the published tree names that `sync.git` lost (its loose object deleted, as damage
/// would) is made again: the publish whose tree could not name it hashes every file once more, as
/// one did before the memo, instead of failing every command after.
#[test]
fn a_published_blob_sync_git_lost_is_made_again() {
    let sandbox = sandbox();
    let origin = sandbox.origin();
    let (_a, ws, _, _b) = set_up(&sandbox);
    let quiet = round(&ws, origin);
    let first = own_segment(&ws, 1);
    let blob = git(
        ["hash-object", ws.dir.join(&first).to_str().expect("UTF-8")],
        None,
    );
    let blob = blob.trim();
    let loose = ws
        .dir
        .join("sync.git/objects")
        .join(&blob[..2])
        .join(&blob[2..]);
    assert!(loose.exists(), "a's own blob is loose in its sync.git");
    std::fs::remove_file(&loose).expect("the loose object");
    let healed = round(&ws, origin);
    assert_eq!(
        healed.hashed,
        journal_files(&ws),
        "every file, hashed again"
    );
    assert_eq!(healed.tree, quiet.tree);
    assert!(loose.exists(), "made again");
    assert_eq!(round(&ws, origin).hashed, Vec::<String>::new());
}
