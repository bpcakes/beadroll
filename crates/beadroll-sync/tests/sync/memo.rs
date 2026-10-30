//! What a publish hashes (roll-lqjj.1): the journal files the store's memo cannot vouch for, each
//! once, and nothing else, while the tree it publishes is the one hashing every file makes; a memo
//! that is missing, stale, unreadable or not past the files it remembers is never trusted.
//!
//! A memo vouches only for a file settled before the memo itself was written, and a command writes
//! the memo in the timestamp tick of the files it just wrote, which the clock an inode's times come
//! from need not have left. A test that measures what a publish hashes therefore moves the memo's
//! own time past every file first ([`settled_round`]), so that what is measured is change alone.

mod trees;

use std::fs::{FileTimes, OpenOptions};
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

use beadroll_core::json::{Object, Value};
use beadroll_store::IdentityMode;
use beadroll_store::{
    Endpoint, Workspace, claim_files, memo_path, relative, replica_id, segment_files,
};
use beadroll_sync::PublishMode;
use beadroll_sync::{Error, Link, PushOutcome, locked, writing};
use beadroll_test_support::{Machine, Sandbox, git};

use crate::support::{actor, claim_command, sandbox, workspace};

/// Which side of the current time a memo timestamp is set to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TimeDirection {
    /// Earlier.
    Earlier,
    /// Later.
    Later,
}

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
        link.publish(tip.as_deref(), "sync", &[], PublishMode::IfChanged)
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

/// Sets the memo's own modification time `from_now` from now: a memo written after every file it
/// remembers vouches for them all, one written before a file for none of what it says of that file.
fn memo_time(ws: &Workspace, from_now: Duration, later: TimeDirection) {
    let later = matches!(later, TimeDirection::Later);
    let file = OpenOptions::new()
        .write(true)
        .open(memo_path(ws))
        .expect("a memo to set the time of");
    let now = SystemTime::now();
    let moment = if later {
        now + from_now
    } else {
        now - from_now
    };
    file.set_times(FileTimes::new().set_modified(moment))
        .expect("the memo's time");
}

/// A round with the memo's own time moved past every file it remembers, so that it is trusted for
/// each and what the round hashes is what changed ([`memo_time`]).
fn settled_round(ws: &Workspace, origin: &Path) -> Round {
    memo_time(ws, Duration::from_secs(60), TimeDirection::Later);
    round(ws, origin)
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
    let me = replica_id(&ws.settings, IdentityMode::Existing)
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
    assert_eq!(settled_round(&ws, origin).hashed, Vec::<String>::new());

    b.bead(["create", "more from b"]);
    let extended = settled_round(&ws, origin);
    assert_eq!(extended.hashed, Vec::<String>::new(), "b's segment came in");
    same_tree_without_memo(&ws, origin, &extended);

    comment(&ws, &a, &key, "one");
    let appended = settled_round(&ws, origin);
    assert_eq!(appended.hashed, [first.as_str()]);
    same_tree_without_memo(&ws, origin, &appended);

    comment(&rotating, &a, &key, "two");
    assert!(ws.dir.join(&second).exists(), "the journal rotated");
    let rotated = settled_round(&ws, origin);
    assert_eq!(
        rotated.hashed,
        [second.as_str()],
        "the full segment is remembered"
    );
    same_tree_without_memo(&ws, origin, &rotated);
    comment(&ws, &a, &key, "three");
    assert_eq!(settled_round(&ws, origin).hashed, [second.as_str()]);

    let full = ws.dir.join(&first);
    let mut torn = OpenOptions::new()
        .append(true)
        .open(&full)
        .expect("the full segment");
    torn.write_all(b"{\"v\": 1, \"id").expect("a torn line");
    drop(torn);
    let repaired = settled_round(&ws, origin);
    assert_eq!(repaired.hashed, [first.as_str()]);
    same_tree_without_memo(&ws, origin, &repaired);

    claim_command(&ws, &actor(&a), &key).expect("a claim");
    let attempts: Vec<String> = claim_files(&ws)
        .expect("the attempt files")
        .iter()
        .map(|file| relative(&ws, file))
        .collect();
    assert_eq!(attempts.len(), 1, "{attempts:?}");
    let claimed = settled_round(&ws, origin);
    assert_eq!(claimed.hashed, attempts);
    same_tree_without_memo(&ws, origin, &claimed);
    assert_eq!(settled_round(&ws, origin).hashed, Vec::<String>::new());
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
    let quiet = settled_round(&ws, origin);
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
        settled_round(&ws, origin).hashed,
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
    let exchanged = settled_round(&ws, origin);
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
    let damaged = settled_round(&ws, origin);
    assert_eq!(damaged.hashed, [foreign.as_str()]);
    assert_eq!(damaged.forks, [foreign.as_str()]);
    assert_eq!(damaged.tree, quiet.tree, "the published copy is back");
}

/// An attempt file damaged here, its length kept, is replaced by the merge with the published
/// copy, and the identity the merge's locked phase took of the damaged one is forgotten with it
/// (roll-lqjj.30): the publish that follows in that phase finds the copy the merge wrote, as the
/// memo remembers it, and hashes nothing more.
#[test]
fn an_attempt_the_merge_replaces_is_known_by_its_new_identity() {
    let sandbox = sandbox();
    let origin = sandbox.origin();
    let (a, ws, key, _b) = set_up(&sandbox);
    claim_command(&ws, &actor(&a), &key).expect("a claim");
    let attempt = relative(&ws, &claim_files(&ws).expect("the attempt files")[0]);
    let quiet = settled_round(&ws, origin);
    assert_eq!(quiet.hashed, [attempt.as_str()]);

    let file = ws.dir.join(&attempt);
    let mut bytes = std::fs::read(&file).expect("the attempt file");
    let at = bytes.iter().position(|b| *b == b'{').expect("a record");
    bytes[at] = b'[';
    std::fs::write(&file, &bytes).expect("damage");
    let damaged = settled_round(&ws, origin);
    assert_eq!(damaged.forks, [attempt.as_str()]);
    assert_eq!(damaged.hashed, [attempt.as_str()], "the damaged copy, once");
    assert_eq!(damaged.tree, quiet.tree, "the published copy is back");
    assert_eq!(settled_round(&ws, origin).hashed, Vec::<String>::new());
}

/// A memo vouches for a file only once the file was settled when the memo was written: a rewrite
/// in place that keeps a file's length can keep every part of its identity, while the times it
/// sets are the ones the file already has. A memo written before a file says nothing of it: the
/// file is hashed again, and the tree published is the one hashing every file makes.
#[test]
fn a_memo_written_before_a_file_says_nothing_of_it() {
    let sandbox = sandbox();
    let origin = sandbox.origin();
    let (_a, ws, _, _b) = set_up(&sandbox);
    let all = journal_files(&ws);
    let quiet = settled_round(&ws, origin);
    assert_eq!(quiet.hashed, Vec::<String>::new(), "every file settled");

    memo_time(&ws, Duration::from_secs(60), TimeDirection::Earlier);
    let racy = round(&ws, origin);
    assert_eq!(racy.hashed, all, "a memo older than the files it names");
    assert_eq!(racy.tree, quiet.tree, "the tree hashing every file makes");
    assert_eq!(
        settled_round(&ws, origin).hashed,
        Vec::<String>::new(),
        "written again, after them"
    );
}

/// This machine's identity.
fn own_machine(ws: &Workspace) -> String {
    replica_id(&ws.settings, IdentityMode::Existing)
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
    // the memo trusted for every file but the one just appended to, which grew
    memo_time(&ws, Duration::from_secs(60), TimeDirection::Later);
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
    let after = settled_round(&ws, origin);
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
    assert_eq!(settled_round(&ws, origin).hashed, Vec::<String>::new());
}
