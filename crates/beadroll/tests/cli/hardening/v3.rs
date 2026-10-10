//! V3: what bead reads that is not the text it expects: non-UTF-8 identity and text on the command
//! line and in the environment, import records whose members have the wrong shape, and a folder of
//! the coordination ref whose name is not UTF-8.

use beadroll_test_support::{Json, json};

/// `_v3_common`: the fields every imported record in these tests carries besides id and title.
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

mod v3_non_utf8_identity_and_text {
    use std::ffi::{OsStr, OsString};
    use std::os::unix::ffi::OsStrExt as _;

    use beadroll_test_support::{Machine, json, parse_json};

    /// A variable bead reads its identity from, and the bytes it holds.
    type Variable<'a> = (&'a str, &'a [u8]);

    /// Verified (V3): non-UTF-8 identity or text arriving through argv or the environment
    /// (--actor, BEAD_THREAD on claim and on update --claim, BEAD_AGENT, BEAD_ACTOR, --author) is a
    /// structured VALIDATION error and writes nothing; F07 holds for argv/env. The review measured
    /// 6 cases on an initialised, synced machine, all rc 1 with
    /// {"error":{"code":"VALIDATION","message":"text is not valid UTF-8"}}, no traceback, export
    /// byte-identical afterwards and info pending_ownership == [].
    #[test]
    fn non_utf8_identity_and_text_are_validation_and_write_nothing() {
        let sandbox = crate::sandbox();
        let machine = Machine::new(&sandbox, "v3-id");
        machine.bead(["init", "--prefix", "ex"]);
        let created = machine.bead(["create", "x"]).expect("the created issue");
        let key = created["id"].as_str().expect("an id").to_owned();
        machine.bead(["sync"]);
        let before = machine.export();
        let key = key.as_bytes();
        let cases: [(&[&[u8]], Option<Variable<'_>>); 6] = [
            (
                &[b"--actor", b"n\xff", b"comments", b"add", key, b"hi"],
                None,
            ),
            (&[b"claim", key], Some(("BEAD_THREAD", b"t\xff"))),
            (&[b"create", b"y"], Some(("BEAD_AGENT", b"a\xff"))),
            (&[b"create", b"y"], Some(("BEAD_ACTOR", b"a\xff"))),
            (
                &[b"comments", b"add", key, b"hi", b"--author", b"z\xff"],
                None,
            ),
            (
                &[b"update", key, b"--claim"],
                Some(("BEAD_THREAD", b"t\xff")),
            ),
        ];
        for (argv, extra) in cases {
            let mut args: Vec<&OsStr> = argv.iter().map(|arg| OsStr::from_bytes(arg)).collect();
            args.push(OsStr::new("--json"));
            let mut invocation = machine.cmd(args);
            if let Some((name, value)) = extra {
                invocation = invocation.env(name, OsString::from(OsStr::from_bytes(value)));
            }
            let run = invocation.run();
            let words: Vec<_> = argv.iter().map(|a| String::from_utf8_lossy(a)).collect();
            let label = match extra {
                Some((name, value)) => {
                    format!(
                        "{} with {name}={}",
                        words.join(" "),
                        String::from_utf8_lossy(value)
                    )
                }
                None => words.join(" "),
            };
            assert!(!run.crashed(), "{label}: {}", run.stderr);
            assert_eq!(run.returncode(), 1, "{label}: {}", run.stderr);
            let error = parse_json(&run.stderr).expect("a JSON error");
            assert_eq!(
                error["error"],
                json!({"code": "VALIDATION", "message": "text is not valid UTF-8"}),
                "{label}"
            );
        }
        assert_eq!(machine.export(), before);
        let info = machine.bead(["info"]).expect("info");
        assert_eq!(info["pending_ownership"], json!([]));
    }
}

mod v3_import_shape_errors {
    use beadroll_core::json::{Format, Object, write};
    use beadroll_test_support::{Json, Machine, code, json};

    /// The test's comment, `{"id": 1, "author": "a", "text": "t", "created_at": None}`, with `id`
    /// and `text` as given.
    fn comment(id: Json, text: Json) -> Json {
        json!({"id": id, "author": "a", "text": text, "created_at": null})
    }

    /// Verified (V3): import records whose labels or dependencies have the wrong container type,
    /// or whose comment members have a wrong-typed id or text, are rejected with VALIDATION (no
    /// traceback) and leave no integrity entries. The review measured 6 single-record files
    /// (labels "a,b" and [1], dependencies as a dict and as a str, comment id 1.5, comment text 5):
    /// all 6 VALIDATION, and info integrity == [] afterwards.
    #[test]
    fn import_shape_errors_are_validation() {
        let sandbox = crate::sandbox();
        let machine = Machine::new(&sandbox, "v3-shape");
        machine.bead(["init", "--prefix", "ex"]);
        let cases = [
            ("labels-str", json!({"labels": "a,b"})),
            ("labels-int", json!({"labels": [1]})),
            ("deps-dict", json!({"dependencies": {"depends_on_id": "x"}})),
            ("deps-str", json!({"dependencies": "x"})),
            (
                "c-id-float",
                json!({"comments": [(comment(Json::Float(1.5), json!("t")))]}),
            ),
            (
                "c-text-int",
                json!({"comments": [(comment(json!(1), json!(5)))]}),
            ),
        ];
        for (name, extra) in cases {
            let mut record = Object::new();
            record.insert("id".into(), Json::from(format!("ex-{name}")));
            record.insert("title".into(), Json::from(name));
            for part in [super::common(), extra] {
                let Json::Object(members) = part else {
                    panic!("{part:?} is not an object")
                };
                record.extend(members);
            }
            let source = sandbox.root().join(format!("{name}.jsonl"));
            let line = write(&Json::Object(record), Format::ASCII).expect("a record");
            std::fs::write(&source, line + "\n").unwrap();
            let run = machine.run(["import", source.to_str().unwrap(), "--json"]);
            assert!(!run.crashed(), "{name}: {}", run.stderr);
            assert_ne!(run.returncode(), 0, "{name}: {}", run.stdout);
            assert_eq!(code(&run), "VALIDATION", "{name}: {}", run.stderr);
        }
        let info = machine.bead(["info"]).expect("info");
        assert_eq!(info["integrity"], json!([]));
        let listed = machine
            .bead(["list", "--all", "--limit", "0"])
            .expect("a list");
        assert_eq!(listed["total"].as_i64(), Some(0), "{listed:?}");
    }
}

mod v3_foreign_folder_default_quote_path {
    use std::ffi::OsStr;
    use std::io::Write as _;
    use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
    use std::path::Path;
    use std::process::{Command, Stdio};

    use beadroll_test_support::Machine;

    /// `_v3_git`: git with byte arguments, so that path names need not be UTF-8, run in `repo` with
    /// `data` on its standard input; what it printed, or the test fails with its standard error.
    fn git(repo: &Path, args: &[&[u8]], data: Option<&[u8]>) -> Vec<u8> {
        let mut command = Command::new("git");
        command
            .args(args.iter().map(|arg| OsStr::from_bytes(arg)))
            .current_dir(repo)
            .stdin(if data.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().expect("git starts");
        if let (Some(mut stdin), Some(data)) = (child.stdin.take(), data) {
            stdin.write_all(data).expect("git reads its input");
        }
        let output = child.wait_with_output().expect("git's output");
        assert!(
            output.status.success(),
            "{command:?} failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }

    /// `bytes` without the whitespace around it: Python's `bytes.strip()`.
    fn strip(bytes: &[u8]) -> Vec<u8> {
        bytes.trim_ascii().to_vec()
    }

    /// The NUL-terminated entries `ls-tree -z` printed, the empty ones left out.
    fn entries(listing: &[u8]) -> Vec<Vec<u8>> {
        listing
            .split(|byte| *byte == 0)
            .filter(|entry| !entry.is_empty())
            .map(<[u8]>::to_vec)
            .collect()
    }

    /// `entries` as `mktree -z` reads them: each one terminated by a NUL.
    fn terminated(entries: &[Vec<u8>]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for entry in entries {
            bytes.extend_from_slice(entry);
            bytes.push(0);
        }
        bytes
    }

    /// The tree entry `040000 tree SHA<TAB>NAME`.
    fn tree_entry(sha: &[u8], name: &[u8]) -> Vec<u8> {
        [b"040000 tree ".as_slice(), sha, b"\t".as_slice(), name].concat()
    }

    /// `_v3_add_foreign_folder`: a commit on the coordination ref of `bare` adding
    /// `events/<name>/000001.jsonl` (empty), as a foreign writer would; `name` is raw bytes and
    /// need not be UTF-8.
    fn add_foreign_folder(bare: &Path, name: &[u8]) {
        let reference: &[u8] = b"refs/beads/journal";
        let tip = strip(&git(bare, &[b"rev-parse", reference], None));
        let root = entries(&git(bare, &[b"ls-tree", b"-z", &tip], None));
        let events_sha = root
            .iter()
            .find(|entry| entry.ends_with(b"\tevents"))
            .map(|entry| {
                let head = entry.split(|byte| *byte == b'\t').next().expect("a mode");
                let words: Vec<&[u8]> = head.split(|byte| *byte == b' ').collect();
                words[2].to_vec()
            })
            .expect("the ref's tree has events");
        let content: &[u8] = b"";
        let blob = strip(&git(
            bare,
            &[b"hash-object", b"-w", b"--stdin"],
            Some(content),
        ));
        let line = [b"100644 blob ".as_slice(), &blob, b"\t000001.jsonl\0"].concat();
        let sub = strip(&git(bare, &[b"mktree", b"-z"], Some(line.as_slice())));
        let mut folders = entries(&git(bare, &[b"ls-tree", b"-z", &events_sha], None));
        folders.push(tree_entry(&sub, name));
        let listing = terminated(&folders);
        let events = strip(&git(bare, &[b"mktree", b"-z"], Some(listing.as_slice())));
        let mut new_root: Vec<Vec<u8>> = root
            .into_iter()
            .filter(|entry| !entry.ends_with(b"\tevents"))
            .collect();
        new_root.push(tree_entry(&events, b"events"));
        let listing = terminated(&new_root);
        let tree = strip(&git(bare, &[b"mktree", b"-z"], Some(listing.as_slice())));
        let commit = strip(&git(
            bare,
            &[
                b"-c",
                b"user.name=x",
                b"-c",
                b"user.email=x@example.invalid",
                b"commit-tree",
                &tree,
                b"-p",
                &tip,
                b"-m",
                b"foreign folder",
            ],
            None,
        ));
        git(bare, &[b"update-ref", reference, &commit], None);
    }

    /// The names in `dir`, as bytes, sorted: `sorted(os.listdir(os.fsencode(dir)))`.
    fn listing(dir: &Path) -> Vec<Vec<u8>> {
        let mut names: Vec<Vec<u8>> = std::fs::read_dir(dir)
            .expect("the events folder")
            .map(|entry| entry.expect("an entry").file_name().into_vec())
            .collect();
        names.sort();
        names
    }

    /// Verified (V3): a coordination-ref folder events/<name>/000001.jsonl whose name is not
    /// UTF-8 (caf\xe9) or holds U+2028 does not crash sync under git's default core.quotePath
    /// (empty GIT_CONFIG_GLOBAL, GIT_CONFIG_NOSYSTEM=1): git ASCII-escapes the path, so it decodes,
    /// and the folder is skipped because the path pattern no longer matches. The review measured
    /// sync rc 0 for both names (the crash needs core.quotePath=false, a separate finding).
    #[test]
    fn non_utf8_ref_folder_does_not_crash_sync_under_default_quotepath() {
        let sandbox = crate::sandbox();
        let machine = Machine::new(&sandbox, "v3-lstree");
        machine.bead(["init", "--prefix", "ex"]);
        machine.bead(["create", "x"]);
        machine.bead(["sync"]);
        let config = sandbox.root().join("gitconfig-empty");
        std::fs::write(&config, "").unwrap();
        let events = machine.store().join("events");
        let folders = listing(&events);
        for name in [b"caf\xe9".as_slice(), "line\u{2028}sep".as_bytes()] {
            add_foreign_folder(sandbox.origin(), name);
            let run = machine
                .cmd(["sync", "--json"])
                .env("GIT_CONFIG_GLOBAL", &config)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .run();
            let shown = String::from_utf8_lossy(name);
            assert!(!run.crashed(), "{shown}: {}", run.stderr);
            assert_eq!(run.returncode(), 0, "{shown}: {}", run.stderr);
            assert_eq!(listing(&events), folders, "{shown}");
        }
    }
}
