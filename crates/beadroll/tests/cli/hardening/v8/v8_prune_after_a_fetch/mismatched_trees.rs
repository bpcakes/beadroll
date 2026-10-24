//! Well-formed trees under another tree's name must not silently remove a machine's journal
//! from the published tip (roll-dih8.31), even when that machine never syncs again.

use super::*;

/// AJ02: a mismatched root fails either the manifest read or the listing. Both failures must
/// rebuild, even when the persistent reader is unavailable, and leave b's journal for a fresh c.
#[test]
fn a_mismatched_root_is_local_damage_with_or_without_its_manifest() {
    for how in [Harm::OtherTree { empty: true }, Harm::OnlyManifest] {
        for kept_reader in [true, false] {
            let sandbox = crate::sandbox();
            let log = sandbox.root().join("root-reads.log");
            let mut tree = String::new();
            let (run, a) = sync_harming_with(&sandbox, TREE, how, |origin, a| {
                tree = plumbing(origin, &["rev-parse", TREE], None);
                if !kept_reader {
                    refuse_kept_reader(&sandbox, a, &log);
                }
            });
            if !kept_reader {
                let reads = std::fs::read_to_string(&log).expect("the tree reader's calls");
                assert!(
                    reads
                        .lines()
                        .any(|line| line == "refused cat-file --batch-command"),
                    "{reads}"
                );
                assert!(
                    reads
                        .lines()
                        .any(|line| line == format!("cat-file tree {tree}")),
                    "{reads}"
                );
            }
            assert_rebuilt_and_synced(&run, &a);
            let c = Machine::new(&sandbox, "c");
            c.bead(["sync"]);
            assert_eq!(a.export(), c.export());
        }
    }
}

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
        |origin, _| {
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

/// The standalone tree read must reject the same mismatch when no persistent reader can answer.
/// Its log proves that this command read the harmed tree through cat-file tree before rebuilding.
#[test]
fn a_mismatched_subtree_without_a_kept_reader_is_local_damage() {
    for empty in [true, false] {
        let sandbox = crate::sandbox();
        let log = sandbox.root().join("tree-reads.log");
        let mut tree = String::new();
        let (run, a) = sync_harming_with(
            &sandbox,
            B_FOLDER,
            Harm::OtherTree { empty },
            |origin, a| {
                let theirs = folders(&a.store().join("sync.git"), "refs/beads/remote");
                let folder = folders(origin, TIP)
                    .into_iter()
                    .find(|folder| !theirs.contains(folder))
                    .expect("b's folder, which a has not fetched");
                tree = plumbing(
                    origin,
                    &["rev-parse", &B_FOLDER.replace("{b}", &folder)],
                    None,
                );
                refuse_kept_reader(&sandbox, a, &log);
            },
        );
        let reads = std::fs::read_to_string(&log).expect("the tree reader's calls");
        assert!(
            reads
                .lines()
                .any(|line| line == "refused cat-file --batch-command"),
            "{reads}"
        );
        assert!(
            reads
                .lines()
                .any(|line| line == format!("cat-file tree {tree}")),
            "{reads}"
        );
        assert_rebuilt_and_synced(&run, &a);
        // b never runs again, so the current published tip must still carry its journal for c.
        let c = Machine::new(&sandbox, "c");
        c.bead(["sync"]);
        assert_eq!(a.export(), c.export());
    }
}

/// Refuses only the persistent object reader, as children.rs refuses kept git processes, and
/// records each standalone tree read. The real git still supplies every object and tree byte.
fn refuse_kept_reader(sandbox: &Sandbox, a: &mut Machine, log: &Path) {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let git = std::env::split_paths(&path)
        .map(|dir| dir.join("git"))
        .find(|git| git.is_file())
        .expect("git on PATH");
    let shims = sandbox.root().join("refusing-reader");
    std::fs::create_dir(&shims).unwrap();
    let script = format!(
        "#!/bin/sh\nif [ \"$1\" = cat-file ] && [ \"$2\" = --batch-command ]; then\n\
         echo 'refused cat-file --batch-command' >> {log}\nexit 1\nfi\n\
         if [ \"$1\" = cat-file ] && [ \"$2\" = tree ]; then\n\
         printf 'cat-file tree %s\\n' \"$3\" >> {log}\nfi\nexec {git} \"$@\"\n",
        log = super::super::quoted(log.as_os_str()),
        git = super::super::quoted(git.as_os_str()),
    );
    std::fs::write(shims.join("git"), script).unwrap();
    std::fs::set_permissions(shims.join("git"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::join_paths(std::iter::once(shims).chain(std::env::split_paths(&path)));
    a.extra.insert("PATH".into(), path.unwrap());
}
