//! Object reads and listings checked against git in both object formats.

use super::*;

fn raw(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).unwrap())
        .collect()
}

fn entry(mode: &str, name: &str, oid: &str) -> Vec<u8> {
    [mode.as_bytes(), b" ", name.as_bytes(), b"\0", &raw(oid)].concat()
}

const BLOB: &str = "ce013625030ba8dba906f756967f9e9ca394464a";

#[test]
fn tree_entries_are_read_as_git_reads_them() {
    let data = [
        entry("100644", "a.json", BLOB),
        entry("40000", "claims", BLOB),
    ]
    .concat();
    let entries = tree_entries(&data, 20).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].mode, 0o100644);
    assert_eq!(entries[1].name, b"claims");
    assert_eq!(hex(entries[1].oid), BLOB);
    assert_eq!(tree_entries(b"", 20), Some(Vec::new()));
    // what git refuses
    for bad in [
        entry(" 100644", "a", BLOB),
        entry("100648", "a", BLOB),
        entry("", "a", BLOB),
        entry("100644", "", BLOB),
        entry("100644", "a", BLOB)[..20].to_vec(),
        [entry("100644", "a", BLOB), b"1".to_vec()].concat(),
    ] {
        assert_eq!(tree_entries(&bad, 20), None, "{bad:?}");
    }
}

#[test]
fn a_commit_names_its_tree_as_git_reads_it() {
    let tree = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
    let good = format!("tree {tree}\nparent {BLOB}\nauthor a <a> 1 +0000\n\nm\n");
    assert_eq!(commit_tree(good.as_bytes(), 40).as_deref(), Some(tree));
    let bare = format!("tree {tree}\nauthor a\n");
    assert_eq!(commit_tree(bare.as_bytes(), 40).as_deref(), Some(tree));
    for bad in [
        format!("tree {tree}\n"),
        format!("tree {}\nauthor a\n", tree.to_uppercase()),
        format!("tree {tree} \nauthor a\n"),
        format!("parent {BLOB}\ntree {tree}\n"),
        format!("tree {tree}\nparent {BLOB}\n"),
        format!("tree {tree}\nparent {BLOB}x\nauthor a\n"),
        format!("tree {tree}\nparent {tree}\nauthor a\n"),
    ] {
        assert_eq!(commit_tree(bad.as_bytes(), 40), None, "{bad:?}");
    }
}

/// `ls-tree -r -z` of `commit` in `env`, read back as rows.
fn ls_tree(env: &Env, commit: &str) -> Vec<(Vec<u8>, String)> {
    let listing = env.git(["ls-tree", "-r", "-z", commit]).check().unwrap();
    let rows = listing
        .stdout
        .split(|b| *b == 0)
        .filter(|row| !row.is_empty());
    rows.map(|row| {
        let tab = row.iter().position(|b| *b == b'\t').unwrap();
        let meta = String::from_utf8(row[..tab].to_vec()).unwrap();
        let sha = meta.split_whitespace().nth(2).unwrap().to_owned();
        (row[tab + 1..].to_vec(), sha)
    })
    .collect()
}

/// A commit of a tree shaped like a published journal, with an entry of every kind git has
/// (executable, link, gitlink, a subtree twice), in `env`'s repository; its name.
fn journal_commit(env: &Env) -> String {
    let blob = |data: &str| {
        let hashed = env.git(["hash-object", "-w", "--stdin"]).input(data);
        hashed.string().unwrap()
    };
    let mktree = |entries: String| env.git(["mktree"]).input(entries).string().unwrap();
    let (a, b) = (blob("a\n"), blob("b\n"));
    let missing = "1".repeat(a.len());
    let claims = mktree(format!("100644 blob {a}\t0190-ab.json\n"));
    let replica = mktree(format!(
        "100644 blob {a}\t000001.jsonl\n100755 blob {b}\trun\n120000 blob {b}\tlink\n\
         040000 tree {claims}\tclaims\n160000 commit {missing}\tmodule\n"
    ));
    let events = mktree(format!(
        "040000 tree {replica}\tm1\n040000 tree {replica}\tm\u{e9}2\n"
    ));
    let top = mktree(format!(
        "040000 tree {events}\tevents\n100644 blob {a}\tmanifest.json\n"
    ));
    let identity = ["-c", "user.name=t", "-c", "user.email=t@example.invalid"];
    let args = identity.into_iter().chain(["commit-tree", &top, "-m", "m"]);
    env.git(args).string().unwrap()
}

#[test]
fn listings_trees_and_names_are_the_ones_git_gives() {
    for format in ["sha1", "sha256"] {
        let (_dir, env) = crate::testing::bare(format);
        let commit = journal_commit(&env);
        let mut objects = Objects::new(&env);
        let tip = objects.get(&commit).unwrap();
        assert_eq!(
            list_tree(&mut objects, &tip).unwrap(),
            ls_tree(&env, &commit)
        );
        let listed = list_tree_with_subtrees(&mut objects, &tip).unwrap();
        for (path, oid) in listed.subtrees {
            let path = String::from_utf8(path).unwrap();
            assert_eq!(
                env.git(["rev-parse", &format!("{commit}:{path}")])
                    .string()
                    .unwrap(),
                oid
            );
        }
        let tree = env.git(["rev-parse", &format!("{commit}^{{tree}}")]);
        assert_eq!(
            root_tree(&mut objects, &tip).unwrap().oid,
            tree.string().unwrap()
        );
        env.git(["update-ref", "refs/beads/x", &commit])
            .check()
            .unwrap();
        assert_eq!(objects.name("refs/beads/x"), Ok(commit.clone()));
        let manifest = objects.get(&format!("{commit}:manifest.json")).unwrap();
        assert_eq!(
            (manifest.kind.as_str(), &manifest.data[..]),
            ("blob", &b"a\n"[..])
        );
        // missing: no answer, and the same git answers the next question
        assert_eq!(objects.get(&"2".repeat(commit.len())), Err(NoAnswer));
        assert_eq!(objects.name("refs/beads/none"), Err(NoAnswer));
        assert_eq!(objects.get(&commit).unwrap().oid, commit);
    }
}

#[test]
fn an_object_that_is_not_what_its_name_says_is_left_to_git() {
    let (dir, env) = crate::testing::bare("sha1");
    let commit = journal_commit(&env);
    let identity = ["-c", "user.name=t", "-c", "user.email=t@example.invalid"];
    let other = env
        .git(
            identity
                .into_iter()
                .chain(["commit-tree", &format!("{commit}^{{tree}}"), "-m", "x"]),
        )
        .string()
        .unwrap();
    let loose = |name: &str| {
        dir.path()
            .join("r.git/objects")
            .join(&name[..2])
            .join(&name[2..])
    };
    // the commit's file now holds another commit, whole and readable
    std::fs::remove_file(loose(&commit)).unwrap();
    std::fs::copy(loose(&other), loose(&commit)).unwrap();
    let mut objects = Objects::new(&env);
    let tip = objects.get(&commit).unwrap();
    assert_eq!(list_tree(&mut objects, &tip), Err(NoAnswer));
    assert_eq!(root_tree(&mut objects, &tip), Err(NoAnswer));
    // git refuses both: the caller's git gives bead the same failure as before
    assert!(
        !env.git(["ls-tree", "-r", "-z", &commit])
            .run()
            .unwrap()
            .success()
    );
    let peeled = env.git(["rev-parse", &format!("{commit}^{{tree}}")]);
    assert!(!peeled.run().unwrap().success());
}

#[test]
fn a_subtree_with_another_trees_content_is_never_listed() {
    for format in ["sha1", "sha256"] {
        for path in ["events", "events/m1", "events/m1/claims"] {
            for empty in [true, false] {
                let (dir, env) = crate::testing::bare(format);
                let commit = journal_commit(&env);
                let before = ls_tree(&env, &commit);
                let tree = env
                    .git(["rev-parse", &format!("{commit}:{path}")])
                    .string()
                    .unwrap();
                let blob = env
                    .git(["rev-parse", &format!("{commit}:manifest.json")])
                    .string()
                    .unwrap();
                let entries = if empty {
                    String::new()
                } else {
                    format!("100644 blob {blob}\tother\n")
                };
                let other = env.git(["mktree"]).input(entries).string().unwrap();
                let loose = |name: &str| {
                    dir.path()
                        .join("r.git/objects")
                        .join(&name[..2])
                        .join(&name[2..])
                };
                std::fs::remove_file(loose(&tree)).unwrap();
                std::fs::copy(loose(&other), loose(&tree)).unwrap();
                // Git still lists the tree successfully, with its original entries missing.
                assert_ne!(ls_tree(&env, &commit), before);
                let mut objects = Objects::new(&env);
                assert!(!objects.get(&tree).unwrap().matches_name());
                let tip = objects.get(&commit).unwrap();
                assert!(tip.matches_name());
                assert_eq!(list_tree(&mut objects, &tip), Err(NoAnswer));
            }
        }
    }
}
