//! G03 (`docs/FINDINGS.md`): an unreadable `meta.json` silently moved coordination to origin (two
//! confirmed owners; the tracker pushed into the code origin).

use beadroll_test_support::{Sandbox, git};

/// `_g03_origin_refs`: the names of the refs the sandbox's remote, the code origin, holds.
fn origin_refs(sandbox: &Sandbox) -> String {
    let origin = sandbox.origin().to_str().expect("a UTF-8 path");
    git(
        ["--git-dir", origin, "for-each-ref", "--format=%(refname)"],
        None,
    )
}

/// G03: without --tracked, the coordination remote named with `init --remote` / `sync --remote`
/// is recorded only in the store's meta.json. Workspace.meta() turns a file it cannot read or parse
/// into {}, so remote() falls back to origin: the claim is settled against origin (two confirmed
/// owners), the tracker is pushed into the code origin, and save_meta rewrites meta.json with
/// origin.
mod g03_damaged_meta_moves_coordination_to_origin {
    use std::fs::Permissions;
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::{Path, PathBuf};

    use beadroll_test_support::{Json, Machine, Sandbox, parse_json};

    use super::origin_refs;

    /// Machine a's tracker coordinates through
    /// `coord.git`, not origin, and holds two issues; machine b synced it naming `coord.git`, which
    /// its store's `meta.json` remembers.
    struct Fixture {
        sandbox: Sandbox,
        coord: PathBuf,
        a: Machine,
        b: Machine,
        key: String,
        other: String,
        meta: PathBuf,
    }

    /// The id of the issue a `bead create --json` made: `result["id"]`.
    fn id(created: Option<Json>) -> String {
        let created = created.expect("bead printed the issue it made");
        created["id"].as_str().expect("an id").to_owned()
    }

    /// The member `key` of `value`; absence fails the test.
    fn member<'v>(value: &'v Json, key: &str) -> &'v Json {
        value
            .get(key)
            .unwrap_or_else(|| panic!("no {key:?} in {value:?}"))
    }

    /// `path` as text, for an argument or a JSON member.
    fn text(path: &Path) -> &str {
        path.to_str().expect("a UTF-8 path")
    }

    /// The JSON document held by path.
    fn read_json(path: &Path) -> Json {
        let read = std::fs::read_to_string(path).unwrap();
        parse_json(&read).unwrap_or_else(|error| panic!("{} ({error}): {read}", path.display()))
    }

    /// The canonical absolute path as text, as the remote the store remembers.
    fn resolved(path: &Path) -> Json {
        Json::from(text(&path.canonicalize().unwrap()))
    }

    impl Fixture {
        /// Builds the coordination fixture.
        fn new() -> Fixture {
            let sandbox = crate::sandbox();
            let coord = sandbox.bare("coord.git");
            let a = Machine::new(&sandbox, "a");
            // coordination remote is not origin
            a.bead(["init", "--prefix", "ex", "--remote", text(&coord)]);
            let key = id(a.bead(["create", "contested"]));
            let other = id(a.bead(["create", "uncontested"]));
            a.bead(["sync"]);
            let b = Machine::new(&sandbox, "b");
            // b remembers coord in meta.json ("last used")
            b.bead(["sync", "--remote", text(&coord)]);
            let meta = b.store().join("meta.json");
            assert_eq!(member(&read_json(&meta), "remote"), &resolved(&coord));
            Fixture {
                sandbox,
                coord,
                a,
                b,
                key,
                other,
                meta,
            }
        }

        /// `_damage`: a hand edit's trailing comma.
        fn damage(&self) {
            let read = std::fs::read_to_string(&self.meta).unwrap();
            let damaged = format!("{},}}\n", read.trim_end().trim_end_matches('}'));
            std::fs::write(&self.meta, damaged).unwrap();
        }

        /// `_contest`: a claims the contested issue through coord; b, whose `meta.json` is damaged,
        /// must neither publish to origin nor confirm a claim of its own, and must name the file.
        fn contest(&self) {
            self.a.bead(["claim", &self.key]); // a holds the issue, confirmed through coord
            let claimed = self.b.run(["claim", &self.key, "--json"]);
            assert!(
                !origin_refs(&self.sandbox).contains("refs/beads/journal"),
                "the tracker was published to the code origin"
            );
            assert_ne!(
                claimed.returncode(),
                0,
                "b confirmed a claim that a holds, by settling it against origin: {}",
                claimed.stdout
            );
            let error = parse_json(&claimed.stderr)
                .unwrap_or_else(|error| panic!("{error}: {}", claimed.stderr));
            let message = member(member(&error, "error"), "message");
            let message = message.as_str().expect("a message");
            assert!(message.contains("meta.json"), "{message}");
        }
    }

    /// Who holds the claim on `issue`, as `show` prints it: `issue["claim"]["owner"]`.
    fn owner(issue: &Json) -> &Json {
        member(member(issue, "claim"), "owner")
    }

    /// G03: a meta.json with a trailing comma silently moves claims to origin; a remote named with
    /// --remote must still work (and be reported as accepted) while the file is damaged.
    #[test]
    fn malformed_meta_does_not_move_coordination_to_origin() {
        let s = Fixture::new();
        s.damage();
        s.contest();
        let damaged = std::fs::read_to_string(&s.meta).unwrap();
        // init does not consult the remote; it must not overwrite what it could not read
        s.b.bead(["init"]);
        assert_eq!(
            std::fs::read_to_string(&s.meta).unwrap(),
            damaged,
            "init replaced a meta.json it could not read"
        );
        let claimed =
            s.b.run(["claim", &s.other, "--remote", text(&s.coord), "--json"]);
        assert_eq!(claimed.returncode(), 0, "{}", claimed.stderr);
        s.a.bead(["sync"]);
        assert_eq!(owner(&s.a.show(&s.other)), owner(&s.b.show(&s.other)));
        assert_eq!(member(&read_json(&s.meta), "remote"), &resolved(&s.coord));
        assert!(!origin_refs(&s.sandbox).contains("refs/beads/journal"));
    }

    /// G03: a meta.json this user cannot read (EACCES) silently moves claims to origin.
    #[test]
    fn unreadable_meta_does_not_move_coordination_to_origin() {
        if rustix::process::geteuid().is_root() {
            eprintln!("skipped: root reads a mode-0 file");
            return;
        }
        let s = Fixture::new();
        std::fs::set_permissions(&s.meta, Permissions::from_mode(0o000)).unwrap();
        s.contest();
        // The `finally`: should `contest` fail, the sandbox's drop removes the file all the same.
        if s.meta.exists() {
            std::fs::set_permissions(&s.meta, Permissions::from_mode(0o600)).unwrap();
        }
    }

    /// G03: an owner's close with a damaged meta.json publishes the tracker to origin; it must be
    /// recorded locally, succeed, and leave origin alone.
    #[test]
    fn owner_close_with_damaged_meta_stays_off_origin() {
        let s = Fixture::new();
        s.b.bead(["claim", &s.key]);
        s.damage();
        let closed = s.b.run(["close", &s.key]);
        assert!(
            !origin_refs(&s.sandbox).contains("refs/beads/journal"),
            "the owner's close published the tracker to the code origin"
        );
        assert_eq!(closed.returncode(), 0, "{}", closed.stderr);
        assert!(
            closed.stdout.contains("not yet published"),
            "{}",
            closed.stdout
        );
        assert_eq!(member(&s.b.show(&s.key), "status"), &Json::from("closed"));
        let info = s.b.run(["info", "--json"]);
        // info diagnoses the file instead of stopping on it
        assert_eq!(info.returncode(), 0, "{}", info.stderr);
        let reported = parse_json(&info.stdout).unwrap();
        let remote = member(&reported, "remote").as_str().expect("a remote");
        assert!(remote.contains("meta.json"), "{remote}");
    }
}
