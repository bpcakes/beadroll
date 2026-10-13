//! Import preserves unknown statuses that bead's commands cannot set. Real Beads exports are also
//! round-tripped through bead and compared with `br` and `bv`. Exported comment ids are checked
//! in beadroll-core's `tests/core/compat.rs`; coordination-ref validation is checked in
//! beadroll-store's `tests/store/compat.rs`.

mod import_dry_run;

mod compat_custom_statuses {
    use std::collections::{BTreeMap, BTreeSet};

    use beadroll_core::json::{Format, Object, write};
    use beadroll_test_support::{Json, Machine, code, json, parse_json};

    /// Spaced, ASCII-escaped JSON.
    fn dumps(value: &Json) -> String {
        write(value, Format::ASCII).expect("a serializable record")
    }

    /// `{"id": id, "title": title, "status": status, **fields}`.
    fn record(id: &str, title: &str, status: &str, fields: &Object) -> Json {
        let mut members = Object::new();
        members.insert("id".to_owned(), Json::from(id));
        members.insert("title".to_owned(), Json::from(title));
        members.insert("status".to_owned(), Json::from(status));
        members.extend(fields.clone());
        Json::from(members)
    }

    /// The status of each exported issue, indexed by id.
    fn statuses(export: &str) -> BTreeMap<String, String> {
        let status = |line: &str| {
            let record = parse_json(line).expect("a record");
            let text = |key: &str| record[key].as_str().expect("a string").to_owned();
            (text("id"), text("status"))
        };
        export.lines().map(status).collect()
    }

    /// Beads tools keep a status they do not know (beads_rust: `Status::Custom`); so does bead's
    /// import, and such an issue is neither ready nor done. bead's own commands still set only
    /// bead's vocabulary. After review 7: the beads_rust tracker on linux-1 could not be imported,
    /// one record's status being "invalid".
    #[test]
    fn imported_custom_statuses_are_kept_and_never_ready() {
        let sandbox = crate::sandbox();
        let base = json!({
            "priority": 2, "issue_type": "task", "created_at": "2026-03-23T02:55:33Z",
            "created_by": "someone", "updated_at": "2026-03-23T02:55:42Z", "source_repo": ".",
            "compaction_level": 0, "original_size": 0
        });
        let base = base.as_object().expect("an object");
        let mut unattributed = base.clone();
        unattributed.shift_remove("created_by");
        let records = [
            record("ex-open", "Open", "open", base),
            record("ex-pin", "Pinned", "pinned", base),
            record("ex-odd", "Test issue", "invalid", &unattributed),
            json!({
                "id": "ex-bad", "title": "Bad", "status": "Not A Status", "issue_type": "task",
                "priority": 2
            }),
        ];
        let source = sandbox.root().join("issues.jsonl");
        let lines: String = records[..3].iter().map(|r| dumps(r) + "\n").collect();
        std::fs::write(&source, lines).expect("the Beads export");
        let source_arg = source.to_str().expect("a UTF-8 path");
        let a = Machine::new(&sandbox, "a");
        let init = a.bead(["init", "--prefix", "ex", "--import", source_arg]);
        assert_eq!(
            init.expect("bead init prints a record")["imported"],
            json!(3)
        );
        let wanted = [
            ("ex-open", "open"),
            ("ex-pin", "pinned"),
            ("ex-odd", "invalid"),
        ];
        let wanted = wanted.map(|(id, status)| (id.to_owned(), status.to_owned()));
        assert_eq!(statuses(&a.export()), BTreeMap::from(wanted));
        assert_eq!(a.ready(), BTreeSet::from(["ex-open".to_owned()]));
        assert_eq!(a.bead(["blocked"]), Some(json!([])));
        let listed = a
            .bead(["list", "--limit", "0"])
            .expect("bead list prints a page");
        let issues = listed["issues"].as_array().expect("a list of issues");
        let ids: BTreeSet<&str> = issues
            .iter()
            .map(|issue| issue["id"].as_str().expect("an id"))
            .collect();
        assert_eq!(ids, BTreeSet::from(["ex-open", "ex-pin", "ex-odd"]));
        assert_eq!(a.show("ex-odd")["status"], json!("invalid"));
        // a record without created_by (28 of beads_rust's 1,031) is not attributed to the importer
        let export = a.export();
        let odd = export.lines().find(|line| line.contains(r#""ex-odd""#));
        let odd = parse_json(odd.expect("ex-odd in the export")).expect("a record");
        assert!(odd.get("created_by").is_none(), "{}", dumps(&odd));
        let shown = a.run(["show", "ex-odd"]);
        assert!(shown.stdout.contains("by unknown"), "{}", shown.stdout);
        a.bead(["update", "ex-odd", "--title", "still custom"]);
        assert_eq!(a.show("ex-odd")["status"], json!("invalid"));
        let refused = a.run(["update", "ex-open", "--status", "invalid", "--json"]);
        assert_eq!(
            (refused.returncode(), code(&refused).as_str()),
            (1, "VALIDATION")
        );
        a.bead(["close", "ex-odd"]);
        assert_eq!(a.show("ex-odd")["status"], json!("closed"));
        std::fs::write(&source, dumps(&records[3]) + "\n").expect("the malformed export");
        let malformed = a.run(["import", source_arg, "--json"]);
        assert_eq!(
            (malformed.returncode(), code(&malformed).as_str()),
            (1, "VALIDATION")
        );
        assert!(
            malformed.stderr.contains("unknown status"),
            "{}",
            malformed.stderr
        );
    }
}

mod compat_audit_real_compatibility {
    use std::ffi::OsString;
    use std::io::Read;
    use std::os::unix::ffi::OsStringExt as _;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::sync::mpsc::{self, Receiver};
    use std::time::{Duration, Instant};

    use beadroll_core::json::{Format, write};
    use beadroll_test_support::{Json, Run, UNDER_TEST, json, parse_json};
    use rustix::fs::{Access, access};

    /// Finds infra/compat_real.py in the source tree used to build the test, searching the crate
    /// directory and its ancestors. Independent of the binary under test.
    fn helper() -> PathBuf {
        let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut candidates = crate_dir
            .ancestors()
            .map(|dir| dir.join("infra").join("compat_real.py"));
        let found = candidates.find(|path| path.is_file());
        found.expect("infra/compat_real.py in the source tree")
    }

    /// Expands `~/rest` using `HOME`, else the password database, removing a trailing slash from
    /// the home before appending `/rest`. Without either home, leaves the path unchanged.
    fn expanded(rest: &str) -> PathBuf {
        let home = std::env::var_os("HOME").or_else(|| {
            let user = nix::unistd::User::from_uid(nix::unistd::Uid::current());
            user.ok().flatten().map(|user| user.dir.into_os_string())
        });
        let Some(home) = home else {
            return Path::new("~").join(rest);
        };
        let mut path = home.into_vec();
        while path.last() == Some(&b'/') {
            path.pop();
        }
        path.push(b'/');
        path.extend_from_slice(rest.as_bytes());
        PathBuf::from(OsString::from_vec(path))
    }

    /// The first executable non-directory named `name` in the test's PATH. An empty or unset PATH
    /// finds nothing.
    fn which(name: &str) -> Option<PathBuf> {
        let path = std::env::var_os("PATH").filter(|path| !path.is_empty())?;
        let mut candidates = std::env::split_paths(&path).map(|dir| dir.join(name));
        candidates
            .find(|file| access(file, Access::EXISTS | Access::EXEC_OK).is_ok() && !file.is_dir())
    }

    /// Whether the tool `name` is here, as the test and the helper look for it: on PATH, or in
    /// `~/.local/bin`.
    fn installed(name: &str) -> bool {
        which(name).is_some() || expanded(&format!(".local/bin/{name}")).exists()
    }

    /// Real exports under `~/Documents/*/.beads/issues.jsonl`, sorted. Include hidden
    /// directories, directory symlinks and any matching filename, including dangling links.
    fn exports() -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(expanded("Documents")) else {
            return Vec::new();
        };
        let dirs = entries.filter_map(Result::ok).map(|entry| entry.path());
        let exports = dirs
            .filter(|dir| dir.is_dir())
            .map(|dir| dir.join(".beads").join("issues.jsonl"));
        let mut found: Vec<PathBuf> = exports
            .filter(|export| export.symlink_metadata().is_ok())
            .collect();
        found.sort();
        found
    }

    /// What a pipe held, handed over by the thread that read it to its end.
    type Drained = Receiver<Vec<u8>>;

    /// A thread that reads `pipe` to its end, and the channel it hands what it read to.
    fn drain(mut pipe: impl Read + Send + 'static) -> Drained {
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = pipe.read_to_end(&mut bytes);
            let _ = sender.send(bytes);
        });
        receiver
    }

    /// What the helper printed on `stream`, once the pipe has ended, by `deadline`, decoded as
    /// `text=True` decodes it.
    fn received(receiver: &Drained, deadline: Instant, stream: &str) -> String {
        let left = deadline.saturating_duration_since(Instant::now());
        let bytes = receiver.recv_timeout(left);
        let bytes = bytes.unwrap_or_else(|_| panic!("the helper's {stream} had not ended in time"));
        String::from_utf8(bytes)
            .unwrap_or_else(|error| panic!("the helper's {stream} is not UTF-8: {error}"))
    }

    /// Runs a command and captures its status and text output. If the process or its output
    /// remains open after `timeout`, kill it and fail the test.
    fn run(mut command: Command, timeout: Duration) -> Run {
        let deadline = Instant::now() + timeout;
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .unwrap_or_else(|error| panic!("cannot start {command:?}: {error}"));
        let stdout = drain(child.stdout.take().expect("the helper's standard output"));
        let stderr = drain(child.stderr.take().expect("the helper's standard error"));
        let status = loop {
            if let Some(status) = child.try_wait().expect("the helper's status") {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("{command:?} had not ended after {timeout:?}");
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        Run {
            status,
            stdout: received(&stdout, deadline, "standard output"),
            stderr: received(&stderr, deadline, "standard error"),
        }
    }

    /// `value[key]`: the member, or the `KeyError` that ends the subtest.
    fn field<'v>(value: &'v Json, key: &str) -> Result<&'v Json, String> {
        value.get(key).ok_or_else(|| format!("KeyError: {key:?}"))
    }

    /// Nothing when `holds`, else the failure `what` that ends the subtest.
    fn check(holds: bool, what: &str) -> Result<(), String> {
        if holds { Ok(()) } else { Err(what.to_owned()) }
    }

    /// The first check that one tracker's result row fails, or `Ok` if all checks pass.
    fn verdict(row: &Json) -> Result<(), String> {
        let zero = json!(0);
        check(field(row, "bp_import_rc")? == &zero, "bp_import_rc != 0")?;
        check(
            field(row, "roundtrip_equal")?.truthy(),
            "not roundtrip_equal",
        )?;
        if field(row, "br_source_import_rc")? == &zero {
            check(field(row, "ready_equal")?.truthy(), "not ready_equal")?;
            check(field(row, "blocked_equal")?.truthy(), "not blocked_equal")?;
        } else {
            let rc = field(row, "br_bp_export_import_rc")?;
            check(rc != &zero, "br_bp_export_import_rc == 0")?;
        }
        let passed = json!({"rc": 0, "valid_json": true});
        for mode in ["triage", "plan", "insights"] {
            let outcome = field(field(row, "bv")?, mode)?;
            let what = format!("bv[{mode:?}] != {{'rc': 0, 'valid_json': True}}");
            check(outcome == &passed, &what)?;
        }
        Ok(())
    }

    /// `value` as JSON text, for a failure's message.
    fn shown(value: &Json) -> String {
        write(value, Format::ASCII).expect("a serializable value")
    }

    /// What the test needs and this machine lacks: `br`, `bv`, or any real export.
    fn missing(exports: &[PathBuf]) -> Vec<&'static str> {
        let tools = ["br", "bv"].into_iter().filter(|tool| !installed(tool));
        let data = exports
            .is_empty()
            .then_some("real Beads exports (~/Documents/*/.beads)");
        tools.chain(data).collect()
    }

    /// Reads source JSONL without modifying it; the helper copies data into scratch repositories
    /// with no remotes.
    ///
    /// `infra/compat_real.py` runs under `python3` from PATH. Without `br` or `bv` on PATH or in
    /// `~/.local/bin`, or without a real export, report a skip on standard error. Check each
    /// tracker separately and fail the test if any row fails.
    #[test]
    #[ignore = "needs br, bv and real Beads exports"]
    fn all_available_real_exports_and_br_bv_consumers() {
        let exports = exports();
        let missing = missing(&exports);
        if !missing.is_empty() {
            eprintln!(
                "skipped: {} unavailable on this machine",
                missing.join(", ")
            );
            return;
        }
        let sandbox = crate::sandbox();
        let tmp = sandbox.root().join("bead-compat-real");
        std::fs::create_dir(&tmp).expect("the helper's scratch directory");
        let script = helper();
        let mut command = Command::new("python3");
        command.arg(&script).arg("--out").arg(&tmp);
        command.env(UNDER_TEST, sandbox.bead());
        let proc = run(command, Duration::from_secs(180));
        assert_eq!(proc.returncode(), 0, "{}", proc.stderr);
        // The helper's last line reports its coverage for manual runs.
        let summary = proc.stdout.lines().last().unwrap_or_default();
        let (script, bead) = (script.display(), sandbox.bead().display());
        eprintln!("{script} with {bead}: {summary}");
        let results = std::fs::read_to_string(tmp.join("results.json"));
        let results = parse_json(&results.expect("the helper's results.json"));
        let results = results.expect("results.json is JSON");
        let rows = results.as_array().expect("a list of trackers");
        assert!(!rows.is_empty(), "no tracker in results.json");
        let mut failures = Vec::new();
        for row in rows {
            // `subTest(tracker=row["name"])`: a row without a name fails the whole test
            let tracker = row.get("name").expect("every row names its tracker");
            if let Err(failed) = verdict(row) {
                let (tracker, row) = (shown(tracker), shown(row));
                failures.push(format!("tracker={tracker}: {failed}\n  {row}"));
            }
        }
        assert!(
            failures.is_empty(),
            "{} of {} trackers failed:\n{}",
            failures.len(),
            rows.len(),
            failures.join("\n")
        );
    }
}
