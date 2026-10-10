//! G27 (`docs/FINDINGS.md`): Link.fetch treats sync.git as ready whenever the directory exists,
//! and creates it with `git init` in the caller's raw environment. GIT_WORK_TREE at first contact
//! makes that init fail after mkdir, an init killed midway leaves the same empty directory, and
//! GIT_OBJECT_DIRECTORY sends sync.git's objects to the caller's directory. Every later sync and
//! claim then fails with REMOTE_UNREACHABLE ('not a git repository'), documented as 'retry later',
//! until the user deletes $BEAD_HOME/<ws>/sync.git, although it holds nothing that is not also in
//! events/ or on the remote.

mod g27_unfinished_sync_git {
    use std::path::{Path, PathBuf};

    use beadroll_test_support::{Machine, Sandbox};

    /// The class's `setUp`: machines a and b, each run of theirs under a scratch global git config
    /// (never the developer's); a publishes its tracker (prefix `wd`) holding one issue, whose key
    /// this returns with them.
    fn set_up(sandbox: &Sandbox) -> (Machine, Machine, String) {
        let config = sandbox.root().join("global.gitconfig");
        std::fs::write(&config, "").expect("the scratch config");
        let (mut a, mut b) = (Machine::new(sandbox, "a"), Machine::new(sandbox, "b"));
        for m in [&mut a, &mut b] {
            m.extra
                .insert("GIT_CONFIG_GLOBAL".into(), config.as_os_str().to_owned());
            m.extra.insert("GIT_CONFIG_NOSYSTEM".into(), "1".into());
        }
        a.bead(["init", "--prefix", "wd"]);
        let created = a
            .bead(["create", "one"])
            .expect("bead create prints the issue");
        let issue = created["id"].as_str().expect("an id").to_owned();
        a.bead(["sync"]);
        (a, b, issue)
    }

    /// `assert_b_works`: after `after`, b's sync and its claim of `issue` succeed, and a, once it
    /// has synced, sees the issue in progress.
    fn assert_b_works(a: &Machine, b: &Machine, issue: &str, after: &str) {
        let sync = b.run(["sync", "--json"]);
        assert_eq!(sync.returncode(), 0, "sync after {after}: {}", sync.stderr);
        let claim = b.run(["claim", issue, "--json"]);
        assert_eq!(
            claim.returncode(),
            0,
            "claim after {after}: {}",
            claim.stderr
        );
        a.bead(["sync"]);
        assert_eq!(
            a.show(issue)["status"].as_str(),
            Some("in_progress"),
            "the claim was not published"
        );
    }

    /// The names of the files anywhere under `dir`: `[p.name for p in dir.rglob("*") if
    /// p.is_file()]`, in the order of a walk that does not enter linked directories.
    fn files_under(dir: &Path) -> Vec<String> {
        let mut names = Vec::new();
        let mut folders: Vec<PathBuf> = vec![dir.to_path_buf()];
        while let Some(folder) = folders.pop() {
            for entry in std::fs::read_dir(&folder).expect("a folder of the walk") {
                let entry = entry.expect("a directory entry");
                let path = entry.path();
                if entry.file_type().expect("its type").is_dir() {
                    folders.push(path);
                } else if path.is_file() {
                    names.push(entry.file_name().to_string_lossy().into_owned());
                }
            }
        }
        names
    }

    /// G27: GIT_WORK_TREE in the environment at first contact (Link.env strips it for every later
    /// git call, but not for the init) leaves an empty sync.git that wedges the store.
    #[test]
    fn git_work_tree_at_first_contact() {
        let sandbox = crate::sandbox();
        let (a, b, issue) = set_up(&sandbox);
        let first = b
            .cmd(["sync", "--json"])
            .env("GIT_WORK_TREE", b.checkout())
            .run();
        assert_eq!(
            first.returncode(),
            0,
            "first sync with GIT_WORK_TREE set: {}",
            first.stderr
        );
        assert_b_works(&a, &b, &issue, "a first contact with GIT_WORK_TREE set");
    }

    /// G27: an empty sync.git (what an init killed or failing after mkdir leaves) is never
    /// completed.
    #[test]
    fn sync_git_left_empty_by_an_interrupted_init_is_completed() {
        let sandbox = crate::sandbox();
        let (a, b, issue) = set_up(&sandbox);
        b.bead(["sync"]);
        let sync_git = b.store().join("sync.git");
        std::fs::remove_dir_all(&sync_git).expect("b's sync.git");
        std::fs::create_dir(&sync_git).expect("an empty sync.git");
        assert_b_works(&a, &b, &issue, "finding sync.git empty");
    }

    /// G27: GIT_OBJECT_DIRECTORY at first contact puts sync.git's objects in the caller's directory
    /// and leaves sync.git without objects/, which every later command then fails on.
    #[test]
    fn caller_object_directory_never_reaches_sync_git() {
        let sandbox = crate::sandbox();
        let (a, b, issue) = set_up(&sandbox);
        let objects = sandbox.root().join("caller-objects");
        std::fs::create_dir(&objects).expect("the caller's object directory");
        let first = b
            .cmd(["sync", "--json"])
            .env("GIT_OBJECT_DIRECTORY", &objects)
            .run();
        assert_eq!(first.returncode(), 0, "{}", first.stderr);
        assert_eq!(
            files_under(&objects),
            Vec::<String>::new(),
            "bead wrote sync.git's objects into the caller's object directory"
        );
        assert_b_works(
            &a,
            &b,
            &issue,
            "a first contact with GIT_OBJECT_DIRECTORY set",
        );
    }

    /// G27: a sync.git with HEAD but no objects/ (what an earlier bead left after a first contact
    /// with GIT_OBJECT_DIRECTORY set) is never completed.
    #[test]
    fn sync_git_without_objects_is_completed() {
        let sandbox = crate::sandbox();
        let (a, b, issue) = set_up(&sandbox);
        b.bead(["sync"]);
        let objects = b.store().join("sync.git").join("objects");
        std::fs::remove_dir_all(objects).expect("sync.git's objects");
        assert_b_works(&a, &b, &issue, "finding sync.git without objects/");
    }
}
