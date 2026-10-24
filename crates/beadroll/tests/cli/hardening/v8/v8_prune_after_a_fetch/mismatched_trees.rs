//! Well-formed trees under another tree's name must not silently remove a machine's journal
//! from the published tip (roll-dih8.31), even when that machine never syncs again.

use super::*;

#[test]
fn a_mismatched_subtree_is_rebuilt_before_a_fresh_machine_syncs() {
    for empty in [true, false] {
        let sandbox = crate::sandbox();
        let (run, a) = sync_harming(&sandbox, B_FOLDER, Harm::OtherTree { empty });
        assert_rebuilt_and_synced(&run, &a);
        // b never runs again: c can recover its issue only from the current published tip.
        let c = Machine::new(&sandbox, "c");
        c.bead(["sync"]);
        assert_eq!(a.export(), c.export());
        a.bead(["sync"]);
        assert_eq!(a.export(), c.export());
    }
}

/// A commit with an uppercase tree name is valid to git, but the persistent listing leaves it
/// to ls-tree before reading any subtree. That fallback must check the subtrees itself.
#[test]
fn a_mismatched_subtree_reached_only_by_git_is_local_damage() {
    let sandbox = crate::sandbox();
    let (run, a) = sync_harming_with(
        &sandbox,
        B_FOLDER,
        Harm::OtherTree { empty: true },
        |origin| {
            let tip = plumbing(origin, &["rev-parse", TIP], None);
            let tree = plumbing(origin, &["rev-parse", TREE], None);
            let commit = plumbing(origin, &["cat-file", "commit", &tip], None);
            let commit = commit.replacen(&tree, &tree.to_uppercase(), 1);
            let made = plumbing(
                origin,
                &["hash-object", "-t", "commit", "-w", "--stdin"],
                Some(&commit),
            );
            plumbing(origin, &["update-ref", TIP, &made, &tip], None);
        },
    );
    assert_rebuilt_and_synced(&run, &a);
    let c = Machine::new(&sandbox, "c");
    c.bead(["sync"]);
    assert_eq!(a.export(), c.export());
}
