//! G47: everything under BEAD_HOME, including files beside segments, is owner-only regardless of
//! umask. The torn-fragment collision test uses the `clock` failpoint.

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

    /// A machine whose bead commands run under umask 022. Set it in a shell that execs bead,
    /// avoiding changes to the test process's shared umask.
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

    /// The last segment of the journal in `dir`, ordered by path.
    fn last_segment(dir: &Path) -> PathBuf {
        let mut segments: Vec<PathBuf> = names(dir)
            .into_iter()
            .filter(|(_, name)| name.ends_with(".jsonl"))
            .map(|(path, _)| path)
            .collect();
        segments.sort();
        segments.pop().expect("a segment")
    }

    /// The files beside every journal's segments whose names hold
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

    /// G47: two torn fragments within one second must both survive. Run two syncs with
    /// clock@always=at(NS), one nanosecond apart within second 1791394588.25. Both map to the
    /// same floating-point second, exposing filename collisions. This build-specific clock test
    /// does not depend on umask.
    #[test]
    fn g47_two_torn_fragments_in_one_second_are_both_kept() {
        let sandbox = crate::sandbox();
        let m = crate::rust_only(&sandbox, "g47-torn");
        m.bead(["init", "--prefix", "ex"]);
        m.bead(["create", "one"]);
        let folder = m.store().join("events").join(replica(&m));
        let segment = last_segment(&folder);
        // each fragment, and the moment the sync that cuts it off reads
        let cuts: [(&[u8], i128); 2] = [
            (br#"{"first"#, 1_791_394_588_250_000_000),
            (br#"{"second"#, 1_791_394_588_250_000_001),
        ];
        for (fragment, at) in cuts {
            let mut file = OpenOptions::new().append(true).open(&segment).unwrap();
            file.write_all(fragment).unwrap();
            drop(file);
            let repaired = m
                .cmd(["sync", "--json"])
                .env("BEAD_FAILPOINTS", format!("clock@always=at({at})"))
                .run();
            assert_eq!(repaired.returncode(), 0, "{}", repaired.stderr);
        }
        let torn = format!("{}.torn-", segment.file_name().unwrap().to_str().unwrap());
        let mut fragments: Vec<(String, Vec<u8>)> = names(&folder)
            .into_iter()
            .filter(|(_, name)| name.starts_with(&torn))
            .map(|(path, name)| (name, std::fs::read(path).unwrap()))
            .collect();
        fragments.sort();
        let (stamped, mut kept): (Vec<String>, Vec<Vec<u8>>) = fragments.into_iter().unzip();
        kept.sort();
        assert_eq!(
            kept,
            [br#"{"first"#.to_vec(), br#"{"second"#.to_vec()],
            "a torn fragment was overwritten by a later one"
        );
        // named by the clock the failpoint set: both were cut off within that one second
        assert_eq!(stamped, cuts.map(|(_, at)| format!("{torn}{at}")));
    }
}
