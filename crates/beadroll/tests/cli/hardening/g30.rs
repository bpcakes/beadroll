//! G30 (`docs/FINDINGS.md`): JSON nested deeper than a reader parses is bounded and scoped. A
//! journal line too deep is one integrity problem, never a crash that takes down every command; a
//! tracked config or a published manifest too deep is a structured error; import refuses nesting
//! that not every machine can read. The class's fourth test, a peer whose reader stops lower
//! keeping up after a deep import elsewhere, ran bead under Python 3.12 or 3.13. It became a
//! mixed-fleet case, Python 3.14 against Rust, whose reading ceilings differ (`docs/RUST-PORT.md`
//! §4.6), until the cutover removed Python's launcher; what remains of it is the peer that keeps
//! working beside a refused deep import, both machines on the one bead
//! (`peer_keeps_working_after_a_deep_import_elsewhere`).

mod g30_deep_json_is_bounded_and_scoped {
    use std::collections::BTreeSet;
    use std::io::Write as _;
    use std::path::Path;
    use std::process::{Command, Stdio};

    use beadroll_core::json::{Format, Object, write};
    use beadroll_test_support::{Json, Machine, Run, Sandbox, code, json, parse_json};

    /// `path` as the text bead is given, as Python's `str(path)` gives it.
    fn text(path: &Path) -> &str {
        path.to_str().expect("a UTF-8 path")
    }

    /// `_g30_common`: a Beads record's required fields.
    fn common() -> Json {
        json!({
            "status": "open",
            "priority": 2,
            "issue_type": "task",
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
            "created_by": "seed",
        })
    }

    /// `json.dumps({"id": KEY, "title": TITLE, **_g30_common()})[:-1]`: the record as Python
    /// writes it, without its closing brace, for more members to follow.
    fn head(key: &str, title: &str) -> String {
        let mut members = Object::new();
        members.insert("id".into(), Json::from(key));
        members.insert("title".into(), Json::from(title));
        if let Json::Object(common) = common() {
            members.extend(common);
        }
        let mut written = write(&Json::Object(members), Format::ASCII).expect("a record");
        written.pop(); // its closing brace
        written
    }

    /// `depth` arrays, each inside the one before: `"[" * depth + "]" * depth`.
    fn nested(depth: usize) -> String {
        format!("{}{}", "[".repeat(depth), "]".repeat(depth))
    }

    /// `_g30_foreign`: nested deeper than any Python's json parser reaches.
    fn foreign() -> String {
        nested(1_000_000)
    }

    /// `_g30_record`: a Beads record whose unknown field nests `depth` arrays (the record itself is
    /// one more level).
    fn record(key: &str, depth: usize, field: &str) -> String {
        format!("{}, \"{field}\": {}}}\n", head(key, "deep"), nested(depth))
    }

    /// `_g30_tail`: the last line of `text` that is not blank.
    fn tail(text: &str) -> &str {
        let mut lines = text.trim().split('\n');
        lines.rfind(|line| !line.trim().is_empty()).unwrap_or("")
    }

    /// The last `count` characters of `text`: Python's `text[-count:]`.
    fn last_chars(text: &str, count: usize) -> String {
        let skip = text.chars().count().saturating_sub(count);
        text.chars().skip(skip).collect()
    }

    /// `_g30_git`: plumbing on the sandbox's bare remote `origin` (`GIT_DIR`), run from inside it
    /// rather than the caller's cwd, with the test's environment less every `GIT_*` variable and
    /// `t` as author and committer, `data` on its standard input; what it printed, stripped. A
    /// failure fails the test.
    fn plumbing(origin: &Path, args: &[&str], data: Option<&[u8]>) -> String {
        let inherited = std::env::vars_os();
        let kept = inherited.filter(|(key, _)| !key.as_encoded_bytes().starts_with(b"GIT_"));
        let mut command = Command::new("git");
        command
            .args(args)
            .current_dir(origin)
            .env_clear()
            .envs(kept)
            .env("GIT_DIR", origin)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
            .stdin(if data.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().expect("git starts");
        let input = child.stdin.take();
        let output = std::thread::scope(|scope| {
            // written from another thread, so that git never waits for us while we wait for it
            if let (Some(mut pipe), Some(data)) = (input, data) {
                scope.spawn(move || pipe.write_all(data));
            }
            child.wait_with_output().expect("git's output")
        });
        assert!(
            output.status.success(),
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        let printed = String::from_utf8(output.stdout).expect("git's output is UTF-8");
        printed.trim().to_owned()
    }

    /// The class's `setUp`: machine `g30`'s tracker (prefix `ex`) holds `ordinary work`.
    fn set_up(sandbox: &Sandbox) -> Machine {
        let m = Machine::new(sandbox, "g30");
        m.bead(["init", "--prefix", "ex"]);
        m.bead(["create", "ordinary work"]);
        m
    }

    /// `assert_no_traceback`: `what`, which `run` ran, did not crash.
    fn assert_no_crash(run: &Run, what: &str) {
        assert!(!run.crashed(), "{what} crashed: {}", tail(&run.stderr));
    }

    /// `assert_works`: `what`, which `run` ran, did not crash and succeeded.
    fn assert_works(run: &Run, what: &str) {
        assert_no_crash(run, what);
        let said = last_chars(&run.stderr, 400);
        assert_eq!(run.returncode(), 0, "{what}: {said}");
    }

    /// G30: one journal line nested deeper than json can parse (foreign segment or attempt file)
    /// is reported as an unreadable line, not a RecursionError that takes down every command.
    #[test]
    fn line_too_deep_for_json_is_one_integrity_problem() {
        let sandbox = crate::sandbox();
        let m = set_up(&sandbox);
        let folder = m.store().join("events").join("fffffff0");
        std::fs::create_dir_all(folder.join("claims")).expect("a foreign replica's folder");
        std::fs::write(folder.join("000001.jsonl"), foreign() + "\n").expect("a deep segment");
        for command in ["list", "ready", "info"] {
            let what = format!("bead {command} with a deep segment line");
            assert_works(&m.run([command, "--json"]), &what);
        }
        let info = m.bead(["info"]).expect("bead info prints a record");
        let integrity = info.get("integrity").and_then(Json::as_array);
        let integrity = integrity.expect("info's integrity rows");
        let at_the_line = |row: &Json| {
            let origin = row.get("origin").and_then(Json::as_str);
            origin.is_some_and(|origin| origin.ends_with("000001.jsonl:1"))
        };
        assert!(integrity.iter().any(at_the_line), "{integrity:?}");
        let attempt = folder
            .join("claims")
            .join("01a1179c-0000-7000-8000-000000000001.json");
        std::fs::write(attempt, foreign() + "\n").expect("a deep attempt file");
        let listed = m.run(["list", "--json"]);
        assert_works(&listed, "bead list with a deep attempt file");
        // an unterminated deep final line is a torn tail to repair, also on sync
        let torn = m.store().join("events").join("fffffff1");
        std::fs::create_dir(&torn).expect("another foreign replica's folder");
        std::fs::write(torn.join("000001.jsonl"), foreign()).expect("a deep unterminated tail");
        let synced = m.run(["sync", "--json"]);
        assert_works(&synced, "bead sync with a deep unterminated tail");
    }

    /// G30: a committed .beadroll.json or a published manifest.json nested too deep for json gives
    /// CONFIG / MANIFEST_MISSING (and help still works), not a traceback.
    #[test]
    fn deep_tracked_config_or_manifest_is_a_structured_error() {
        let sandbox = crate::sandbox();
        let m = set_up(&sandbox);
        m.bead(["sync"]);
        let origin = sandbox.origin();
        let tip = plumbing(origin, &["rev-parse", "refs/beads/journal"], None);
        let deep = foreign();
        let blob = plumbing(
            origin,
            &["hash-object", "-w", "--stdin"],
            Some(deep.as_bytes()),
        );
        let listing = format!("100644 blob {blob}\tmanifest.json\n");
        let tree = plumbing(origin, &["mktree"], Some(listing.as_bytes()));
        let commit_tree = ["commit-tree", &tree, "-p", &tip, "-m", "deep manifest"];
        let commit = plumbing(origin, &commit_tree, None);
        plumbing(
            origin,
            &["update-ref", "refs/beads/journal", &commit, &tip],
            None,
        );
        let sync = m.run(["sync", "--json"]);
        assert_no_crash(&sync, "sync against a deep manifest");
        assert_eq!(code(&sync), "MANIFEST_MISSING");

        let config = format!("{{\"workspace\": \"ws-g30-0001\", \"x\": {deep}}}");
        std::fs::write(m.checkout().join(".beadroll.json"), config).expect("a deep config");
        let listed = m.run(["list", "--json"]);
        assert_no_crash(&listed, "list beside a deep .beadroll.json");
        assert_eq!(code(&listed), "CONFIG");
        let help = m.run(["help"]);
        assert_works(&help, "bead help beside a deep .beadroll.json");
    }

    /// G30: import accepts a record nested 20,000 deep (Python 3.14 parses it, 3.12/3.13 cannot),
    /// then every machine replays the event; a bounded depth must be refused whole with
    /// VALIDATION.
    #[test]
    fn import_refuses_nesting_that_not_every_machine_can_read() {
        let sandbox = crate::sandbox();
        let m = set_up(&sandbox);
        let dependency = format!(
            "{}, \"dependencies\": [{{\"issue_id\": \"ex-depmeta\", \"depends_on_id\": \
             \"ex-elsewhere\", \"type\": \"blocks\", \"metadata\": {}}}]}}\n",
            head("ex-depmeta", "m"),
            nested(200)
        );
        let cases = [
            ("deep-20000", record("ex-deep", 20_000, "x_state")),
            ("deep-200", record("ex-deep200", 200, "x_state")),
            ("deep-dep-metadata", dependency),
        ];
        for (name, contents) in cases {
            let path = sandbox.root().join(format!("{name}.jsonl"));
            std::fs::write(&path, contents).expect("the records to import");
            let run = m.run(["import", text(&path), "--json"]);
            assert_no_crash(&run, &format!("import of {name}"));
            assert_ne!(
                run.returncode(),
                0,
                "import of {name} was accepted: {}",
                run.stdout
            );
            assert_eq!(code(&run), "VALIDATION", "{name}");
        }
        let listed = m.bead(["list", "--all", "--limit", "0"]);
        let listed = listed.expect("bead list prints a record");
        let issues = listed["issues"].as_array().expect("a list of issues");
        let titles: BTreeSet<&str> = issues
            .iter()
            .map(|issue| issue["title"].as_str().expect("a title"))
            .collect();
        assert_eq!(titles, BTreeSet::from(["ordinary work"]));
        let ordinary = sandbox.root().join("nested-50.jsonl");
        let nested_50 = record("ex-nested", 50, "custom");
        std::fs::write(&ordinary, nested_50).expect("the record to import");
        let imported = m.bead(["import", text(&ordinary)]);
        let imported = imported.expect("bead import prints a record");
        assert_eq!(imported["imported"].as_i64(), Some(1));
        let export = m.export();
        let row = export.split('\n').find(|row| row.contains("\"ex-nested\""));
        let exported = parse_json(row.expect("the record in the export")).expect("a record");
        let custom = exported.get("custom").expect("its custom field");
        let written = write(custom, Format::ASCII).expect("the field");
        assert_eq!(written, nested(50));
    }

    /// The ids of the issues a `bead list --json` printed: `{i["id"] for i in listed["issues"]}`.
    fn ids(listed: &Json) -> BTreeSet<String> {
        let issues = listed["issues"].as_array().expect("a list of issues");
        let id = |issue: &Json| issue["id"].as_str().expect("an id").to_owned();
        issues.iter().map(id).collect()
    }

    /// G30: after one machine tries to import a record 12,000 levels deep, refused whole, a peer
    /// keeps working and sees what the first sees. In the Python test
    /// (`test_peer_on_python_312_keeps_working_after_a_deep_import_elsewhere`) the peer's reader
    /// stopped lower than the importer's, and until the cutover Python's bead and the Rust one
    /// stood each way round.
    #[test]
    fn peer_keeps_working_after_a_deep_import_elsewhere() {
        let sandbox = crate::sandbox();
        let a = set_up(&sandbox);
        a.bead(["sync"]);
        let b = Machine::new(&sandbox, "g30b");
        assert_works(&b.run(["sync", "--json"]), "the peer's first sync");
        let path = sandbox.root().join("deep.jsonl");
        std::fs::write(&path, record("ex-deep", 12_000, "x_state")).expect("the record");
        let imported = a.run(["import", text(&path), "--json"]);
        assert_no_crash(&imported, "the deep import");
        let accepted = imported.returncode() == 0;
        assert!(
            !accepted,
            "the deep record was imported: {}",
            imported.stdout
        );
        assert_eq!(code(&imported), "VALIDATION", "the deep import");
        a.bead(["create", "made after the import"]);
        a.bead(["sync"]);
        let synced = b.run(["sync", "--json"]);
        assert_works(&synced, "the peer's sync after the import");
        let listed = b.run(["list", "--all", "--limit", "0", "--json"]);
        assert_works(&listed, "the peer's list");
        let seen = parse_json(&listed.stdout).expect("bead list prints a record");
        let first = a.bead(["list", "--all", "--limit", "0"]);
        assert_eq!(ids(&seen), ids(&first.expect("bead list prints a record")));
        assert_eq!(ids(&seen).len(), 2, "{seen:?}");
    }
}
