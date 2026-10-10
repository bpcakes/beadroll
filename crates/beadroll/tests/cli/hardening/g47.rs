//! G47: everything under BEAD_HOME is owner-only whatever the umask, the files bead writes beside
//! a store's segments included. The third test, two torn fragments within one second, needs the
//! `clock` failpoint, and is ported with it.

/// G47: the store is not 'private by its own modes (0700/0600)': events/, events/<m>/, claims/,
/// pending/, lock, *.torn-*, *.fork-* and all of sync.git (FETCH_HEAD with the remote URL, the
/// loose objects holding every event) follow the umask; only BEAD_HOME and <ws> are chmodded
/// 0700.
mod g47_store_is_private_by_its_own_modes {
    use std::fs::{OpenOptions, Permissions};
    use std::io::Write as _;
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::{Path, PathBuf};

    use beadroll_test_support::{Json, Machine, Sandbox, parse_json};

    /// `path` quoted for the shell.
    fn quoted(path: &Path) -> String {
        let text = path.to_str().expect("a UTF-8 path");
        format!("'{}'", text.replace('\'', r"'\''"))
    }

    /// A machine `name` whose every bead runs under umask 022, as the class's `setUp` has it. The
    /// Python test sets its own process's umask, which every bead it starts inherits; a Rust test
    /// may share its process with the tests running beside it, so here bead starts through a shell
    /// that sets the umask and then becomes the sandbox's bead.
    fn machine(sandbox: &Sandbox, name: &str) -> Machine {
        let wrapper = sandbox.root().join("bead-umask-022");
        if !wrapper.exists() {
            let script = format!(
                "#!/bin/sh\numask 022 && exec {} \"$@\"\n",
                quoted(sandbox.bead())
            );
            std::fs::write(&wrapper, script).unwrap();
            std::fs::set_permissions(&wrapper, Permissions::from_mode(0o755)).unwrap();
        }
        Machine::new(sandbox, name).with_bead(wrapper)
    }

    /// Every path under `dir`, depth first, symbolic links not followed: `Path.rglob("*")`.
    fn descendants(dir: &Path, found: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("a directory") {
            let path = entry.expect("a directory entry").path();
            found.push(path.clone());
            if std::fs::symlink_metadata(&path).unwrap().is_dir() {
                descendants(&path, found);
            }
        }
    }

    /// `_g47_loose`: every path under `root` (`root` included) that group or others can read,
    /// write or enter, with its mode.
    fn loose(root: &Path) -> Vec<String> {
        let mut paths = vec![root.to_path_buf()];
        descendants(root, &mut paths);
        let mut out = Vec::new();
        for path in paths {
            let mode = std::fs::symlink_metadata(&path)
                .unwrap()
                .permissions()
                .mode();
            if mode & 0o7777 & 0o077 != 0 {
                let relative = path.strip_prefix(root).unwrap();
                out.push(format!("{mode:o} {}", relative.display()));
            }
        }
        out
    }

    /// The id in `machine`'s `replica.json`.
    fn replica(machine: &Machine) -> String {
        let text = std::fs::read_to_string(machine.home().join("replica.json")).unwrap();
        let identity = parse_json(&text).unwrap_or_else(|error| panic!("{error}: {text}"));
        let id = identity.get("id").and_then(Json::as_str);
        id.unwrap_or_else(|| panic!("no id in {text}")).to_owned()
    }

    /// The names of the entries of `dir`.
    fn names(dir: &Path) -> Vec<(PathBuf, String)> {
        let entries = std::fs::read_dir(dir).expect("a directory");
        entries
            .map(|entry| {
                let path = entry.expect("a directory entry").path();
                let name = path.file_name().unwrap().to_str().unwrap().to_owned();
                (path, name)
            })
            .collect()
    }

    /// `sorted(dir.glob("*.jsonl"))[-1]`: the last segment of the journal in `dir`.
    fn last_segment(dir: &Path) -> PathBuf {
        let mut segments: Vec<PathBuf> = names(dir)
            .into_iter()
            .filter(|(_, name)| name.ends_with(".jsonl"))
            .map(|(path, _)| path)
            .collect();
        segments.sort();
        segments.pop().expect("a segment")
    }

    /// `store.glob("events/*/*MARK*")`: the files beside every journal's segments whose names hold
    /// `mark`.
    fn beside_segments(store: &Path, mark: &str) -> Vec<PathBuf> {
        let mut found = Vec::new();
        for (journal, _) in names(&store.join("events")) {
            if journal.is_dir() {
                let marked = names(&journal)
                    .into_iter()
                    .filter(|(_, n)| n.contains(mark));
                found.extend(marked.map(|(path, _)| path));
            }
        }
        found
    }

    /// G47: after init, create, an online claim, syncs with a second machine, a torn tail and a
    /// damaged foreign segment, every path under BEAD_HOME is owner-only (0700 directories,
    /// 0600/0400 files).
    #[test]
    fn g47_every_store_path_is_owner_only_under_umask_022() {
        let sandbox = crate::sandbox();
        let (a, b) = (machine(&sandbox, "g47-a"), machine(&sandbox, "g47-b"));
        let origin = sandbox.origin().to_str().expect("a UTF-8 path");
        a.bead(["init", "--prefix", "ex", "--remote", origin]);
        let created = a.bead(["create", "one"]).expect("create prints the issue");
        let key = created["id"].as_str().expect("the issue's id").to_owned();
        a.bead(["claim", &key]); // online: sync.git, claims/, pending/
        b.bead(["sync"]);
        b.bead(["create", "from b"]);
        b.bead(["sync"]);
        a.bead(["sync"]);
        let (own, foreign) = (replica(&a), replica(&b));
        let events = a.store().join("events");
        let segment = last_segment(&events.join(&own));
        let mut file = OpenOptions::new().append(true).open(&segment).unwrap();
        file.write_all(br#"{"torn"#).unwrap();
        drop(file);
        a.bead(["create", "after torn"]); // *.torn-*
        let theirs = last_segment(&events.join(&foreign));
        let text = std::fs::read_to_string(&theirs).unwrap();
        std::fs::write(&theirs, text.replace("from b", "FROM B")).unwrap();
        a.bead(["sync"]); // *.fork-*
        let store = a.store();
        assert!(
            !beside_segments(&store, ".torn-").is_empty(),
            "the scenario made no torn fragment"
        );
        assert!(
            !beside_segments(&store, ".fork-").is_empty(),
            "the scenario made no fork copy"
        );
        let loose = loose(a.home());
        assert!(
            loose.is_empty(),
            "{} store paths are not owner-only: {loose:#?}",
            loose.len()
        );
    }

    /// G47: `machines --name` as the first command in a fresh BEAD_HOME creates it 0755.
    #[test]
    fn g47_machines_name_as_first_command_makes_a_private_home() {
        let sandbox = crate::sandbox();
        let m = machine(&sandbox, "g47-first");
        let named = m.run(["machines", "--name", "deadbeef=laptop", "--json"]);
        assert_eq!(named.returncode(), 0, "{}", named.stderr);
        assert_eq!(loose(m.home()), Vec::<String>::new());
    }
}
