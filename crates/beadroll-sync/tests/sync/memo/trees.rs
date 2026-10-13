//! Subtree reuse changes neither the published bytes nor which local attempts get published.

use beadroll_store::{attempt_path, encode_events};

use super::*;

/// The tree a fresh link builds with no merged tip to vouch for any subtree. The files are already
/// merged locally: the only difference from round() is that every tree must go through mktree.
fn rebuilt_tree(ws: &Workspace, origin: &Path) -> String {
    let mut link = Link::new(ws, &Endpoint::at(origin)).unwrap();
    let tip = link.fetch().unwrap();
    locked(ws, || link.publish(tip.as_deref(), "sync", &[], false)).unwrap();
    assert!(link.reused_trees.is_empty());
    link.env
        .git([
            "rev-parse",
            &format!("{}^{{tree}}", link.last_commit.unwrap()),
        ])
        .string()
        .unwrap()
}

#[test]
fn unchanged_claims_reuse_the_merged_tree_and_changed_claims_are_rebuilt() {
    let sandbox = sandbox();
    let origin = sandbox.origin();
    let (a, ws, key, b) = set_up(&sandbox);
    claim_command(&ws, &actor(&a), &key).unwrap();
    let path = format!("events/{}/claims", own_machine(&ws));
    let quiet = round(&ws, origin);
    assert_eq!(quiet.reused_trees, [path.as_str()]);
    assert_eq!(quiet.tree, rebuilt_tree(&ws, origin));

    // A write to a segment leaves all claims unchanged.
    comment(&ws, &a, &key, "next");
    let written = round(&ws, origin);
    assert_eq!(written.reused_trees, [path.as_str()]);
    assert_eq!(written.tree, rebuilt_tree(&ws, origin));

    // An accepted attempt brought by the merge is available to the very next publish, despite
    // the listing the merge read before writing it.
    let other = b.bead(["create", "b's claim"]).unwrap();
    b.bead(["claim", other["id"].as_str().unwrap()]);
    let merged = round(&ws, origin);
    assert_eq!(merged.reused_trees.len(), 2);
    assert_eq!(merged.tree, rebuilt_tree(&ws, origin));

    // A local attempt not in the merged tip changes that directory; the other one still reuses.
    a.bead(["release", &key, "--offline"]);
    let changed = round(&ws, origin);
    assert_eq!(changed.reused_trees.len(), 1);
    assert_ne!(changed.reused_trees[0], path);
    assert_eq!(changed.tree, rebuilt_tree(&ws, origin));
}

#[test]
fn a_pending_attempt_changes_the_tree_even_when_all_accepted_files_are_unchanged() {
    let sandbox = sandbox();
    let origin = sandbox.origin();
    let (a, ws, key, _b) = set_up(&sandbox);
    claim_command(&ws, &actor(&a), &key).unwrap();
    let before = round(&ws, origin);
    let accepted = claim_files(&ws).unwrap();
    let mut pending = beadroll_store::read_attempt(&accepted[0], None);
    // A distinct attempt name is enough to check publication's tree union. The protocol tests
    // exercise valid event generation and confirmation; this test compares the exact tree bytes.
    let Value::Object(event) = &mut pending[0] else {
        panic!("an event")
    };
    event.insert(
        "id".into(),
        Value::from("01999999-9999-7999-8999-999999999999"),
    );
    let mut link = Link::new(&ws, &Endpoint::at(origin)).unwrap();
    let tip = link.fetch().unwrap();
    locked(&ws, || -> Result<(), Error> {
        link.merge(tip.as_deref())?;
        assert_eq!(
            link.publish(tip.as_deref(), "claim", &pending, false)?,
            PushOutcome::Ok
        );
        Ok(())
    })
    .unwrap();
    assert!(link.reused_trees.is_empty());
    let commit = link.last_commit.unwrap();
    let tree = link
        .env
        .git(["rev-parse", &format!("{commit}^{{tree}}")])
        .string()
        .unwrap();
    assert_ne!(tree, before.tree);
    let path = relative(&ws, &attempt_path(&ws, &pending));
    let bytes = link
        .env
        .git(["show", &format!("{commit}:{path}")])
        .run()
        .unwrap()
        .stdout;
    assert_eq!(bytes, encode_events(&pending).unwrap());
}

#[test]
fn a_new_merge_forgets_subtrees_no_longer_reachable_from_its_tip() {
    let sandbox = sandbox();
    let origin = sandbox.origin();
    let (a, ws, key, _b) = set_up(&sandbox);
    let mut link = Link::new(&ws, &Endpoint::at(origin)).unwrap();
    let earlier = link.fetch().unwrap();
    claim_command(&ws, &actor(&a), &key).unwrap();
    let tip = link.fetch().unwrap();
    locked(&ws, || link.merge(tip.as_deref())).unwrap();
    let spec = format!(
        "{}:events/{}/claims",
        tip.as_deref().unwrap(),
        own_machine(&ws)
    );
    let forgotten = link.env.git(["rev-parse", &spec]).string().unwrap();
    // Rewind the throwaway remote and merge the earlier tip through the same link. No tree from
    // the earlier merge may vouch for this publication, even if the local files still match it.
    git(
        [
            "--git-dir",
            origin.to_str().unwrap(),
            "update-ref",
            &link.ref_name,
            earlier.as_deref().unwrap(),
        ],
        None,
    );
    let tip = link.fetch().unwrap();
    locked(&ws, || -> Result<(), Error> {
        link.merge(tip.as_deref())?;
        link.env
            .git(["gc", "--prune=now", "--quiet"])
            .check()
            .unwrap();
        assert!(
            !link
                .env
                .git(["cat-file", "-e", &forgotten])
                .run()
                .unwrap()
                .success()
        );
        assert_eq!(
            link.publish(tip.as_deref(), "sync", &[], false)?,
            PushOutcome::Ok
        );
        Ok(())
    })
    .unwrap();
    assert!(link.reused_trees.is_empty());
    assert_eq!(round(&ws, origin).tree, rebuilt_tree(&ws, origin));
}

#[test]
fn a_missing_reused_tree_is_rebuilt_from_local_files() {
    let sandbox = sandbox();
    let origin = sandbox.origin();
    let (a, ws, key, _b) = set_up(&sandbox);
    claim_command(&ws, &actor(&a), &key).unwrap();
    let quiet = round(&ws, origin);
    let mut link = Link::new(&ws, &Endpoint::at(origin)).unwrap();
    let tip = link.fetch().unwrap();
    locked(&ws, || -> Result<(), Error> {
        link.merge(tip.as_deref())?;
        let spec = format!(
            "{}:events/{}/claims",
            tip.as_deref().unwrap(),
            own_machine(&ws)
        );
        let tree = link.env.git(["rev-parse", &spec]).string().unwrap();
        // Every file's identity changes, so the publish hashes all its blobs afresh. It still
        // needs to recover the reused tree: blob rehashing alone cannot make that tree exist.
        for file in journal_files(&ws) {
            let file = ws.dir.join(file);
            let bytes = std::fs::read(&file).unwrap();
            std::fs::write(file, bytes).unwrap();
        }
        let loose = link
            .git_dir
            .join("objects")
            .join(&tree[..2])
            .join(&tree[2..]);
        std::fs::remove_file(&loose).unwrap();
        assert_eq!(
            link.publish(tip.as_deref(), "sync", &[], false)?,
            PushOutcome::Ok
        );
        assert!(loose.exists(), "the claims tree was made again");
        Ok(())
    })
    .unwrap();
    assert_eq!(quiet.tree, rebuilt_tree(&ws, origin));
}
