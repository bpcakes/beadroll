//! Expected handling of arguments and aliases that are not UTF-8, normalised paths and an
//! unavailable TMPDIR. Fixed outputs substitute `{key}` for the issue created by the test.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt as _;
use std::path::Path;

use beadroll_test_support::{Json, Machine};

use super::{id, json};

/// How a run ended: its exit status, standard output and standard error.
type Answer = (i32, String, String);

/// `bead ARGS` on `machine`, each argument the bytes given, run in `dir` or else its checkout,
/// `{key}` replaced by `key`.
fn answer(machine: &Machine, dir: Option<&Path>, key: &str, args: &[&[u8]]) -> Answer {
    let args: Vec<Vec<u8>> = args.iter().map(|a| keyed(a, key)).collect();
    let mut command = machine.cmd(args.iter().map(|a| OsStr::from_bytes(a)));
    if let Some(dir) = dir {
        command = command.cwd(dir);
    }
    let run = command.run();
    (run.returncode(), run.stdout, run.stderr)
}

/// `bytes` with every `{key}` replaced by `key`.
fn keyed(bytes: &[u8], key: &str) -> Vec<u8> {
    let text = String::from_utf8_lossy(bytes);
    if !text.contains("{key}") {
        return bytes.to_vec();
    }
    text.replace("{key}", key).into_bytes()
}

/// Expected status, stdout and stderr, with `{key}` substituted.
fn said(status: i32, stdout: &str, stderr: &str, key: &str) -> Answer {
    let line = |text: &str| {
        if text.is_empty() {
            String::new()
        } else {
            format!("{}\n", text.replace("{key}", key))
        }
    };
    (status, line(stdout), line(stderr))
}

/// A command line and its expected human and JSON status, stdout and stderr, with line endings
/// omitted.
type Case = (
    &'static [&'static [u8]],
    (i32, &'static str, &'static str),
    (i32, &'static str, &'static str),
);

/// The invalid-UTF-8 error in both output modes.
const NOT_UTF8: (i32, &str, &str) = (1, "", "Error: text is not valid UTF-8");
const NOT_UTF8_JSON: (i32, &str, &str) = (
    1,
    "",
    r#"{"error": {"code": "VALIDATION", "message": "text is not valid UTF-8"}}"#,
);

/// The arguments that are not UTF-8, once the tracker and `{key}` exist.
const ARGUMENTS: [Case; 14] = [
    (
        &[b"create", b"t", b"x\xff"],
        (2, "", "Error: unrecognized arguments: x?"),
        (
            2,
            "",
            r#"{"error": {"code": "USAGE", "message": "unrecognized arguments: x\udcff"}}"#,
        ),
    ),
    (
        &[b"create", b"--de=\xff"],
        (
            2,
            "",
            "Error: ambiguous option: --de=? could match --description, --description-file, \
             --design, --deps",
        ),
        (
            2,
            "",
            r#"{"error": {"code": "USAGE", "message": "ambiguous option: --de=\udcff could match --description, --description-file, --design, --deps"}}"#,
        ),
    ),
    (
        &[b"show", b"ex-\xff"],
        (1, "", "Error: Issue not found: ex-?"),
        (
            1,
            "",
            r#"{"error": {"code": "ISSUE_NOT_FOUND", "message": "Issue not found: ex-\udcff"}}"#,
        ),
    ),
    (
        &[b"update", b"\xff", b"--title", b"t"],
        (1, "", "Error: Issue not found: ?"),
        (
            1,
            "",
            r#"{"error": {"code": "ISSUE_NOT_FOUND", "message": "Issue not found: \udcff"}}"#,
        ),
    ),
    (
        &[b"update", b"{key}", b"--title", b"\xff"],
        NOT_UTF8,
        NOT_UTF8_JSON,
    ),
    (
        &[
            b"update",
            b"{key}",
            b"--add-label",
            b"a,\xff",
            b"--status",
            b"bogus",
        ],
        (1, "", "Error: unknown status 'bogus'"),
        (
            1,
            "",
            r#"{"error": {"code": "VALIDATION", "message": "unknown status 'bogus'"}}"#,
        ),
    ),
    (
        &[b"comments", b"add", b"{key}", b"\xff"],
        NOT_UTF8,
        NOT_UTF8_JSON,
    ),
    (
        &[b"create", b"t", b"--description-file", b"/nonexistent/\xff"],
        (1, "", "Error: No such file or directory: /nonexistent/?"),
        (
            1,
            "",
            r#"{"error": {"code": "IO", "message": "No such file or directory: /nonexistent/\udcff"}}"#,
        ),
    ),
    (
        &[b"help", b"\xff"],
        (2, "", r"Error: unknown command '\udcff'; try `bead help`"),
        (
            2,
            "",
            r#"{"error": {"code": "USAGE", "message": "unknown command '\\udcff'; try `bead help`"}}"#,
        ),
    ),
    (
        &[b"list", b"--limit", b"\xff"],
        (
            2,
            "",
            r"Error: argument --limit: invalid int value: '\udcff'",
        ),
        (
            2,
            "",
            r#"{"error": {"code": "USAGE", "message": "argument --limit: invalid int value: '\\udcff'"}}"#,
        ),
    ),
    (
        &[b"dep", b"add", b"{key}", b"ex-\xff"],
        (1, "", "Error: Issue not found: ex-?"),
        (
            1,
            "",
            r#"{"error": {"code": "ISSUE_NOT_FOUND", "message": "Issue not found: ex-\udcff"}}"#,
        ),
    ),
    (
        &[b"dep", b"--type", b"\xff", b"add", b"{key}", b"{key}"],
        (
            1,
            "",
            "Error: {key} -> {key} would create a dependency cycle",
        ),
        (
            1,
            "",
            r#"{"error": {"code": "CYCLE", "message": "{key} -> {key} would create a dependency cycle"}}"#,
        ),
    ),
    (&[b"search", b"\xff"], (0, "No matches", ""), (0, "[]", "")),
    (
        &[b"close", b"{key}", b"-r", b"\xff"],
        NOT_UTF8,
        NOT_UTF8_JSON,
    ),
];

/// Runs `case` on `machine` in human and JSON modes, in `dir` or the checkout, and compares with
/// the expected results.
fn ask(machine: &Machine, dir: Option<&Path>, key: &str, case: &Case) {
    let (args, human, json) = case;
    for ((status, stdout, stderr), json_mode) in [(human, false), (json, true)] {
        let mut all = args.to_vec();
        if json_mode {
            all.push(b"--json");
        }
        let shown: Vec<_> = all.iter().map(|a| String::from_utf8_lossy(a)).collect();
        assert_eq!(
            answer(machine, dir, key, &all),
            said(*status, stdout, stderr, key),
            "{shown:?}"
        );
    }
}

/// Invalid UTF-8 arguments match no key and fail only when written, after other validation.
/// Errors show `?` for each invalid byte in human output and `\udcXX` in JSON. Nothing is
/// written.
#[test]
fn arguments_that_are_not_utf8_keep_established_diagnostics() {
    let sandbox = crate::sandbox();
    let m = Machine::new(&sandbox, "bytes");
    // A missing tracker is reported before an unwritable title, with sync as the first recovery
    // step.
    let uninitialised: Case = (
        &[b"create", b"t\xff"],
        (
            1,
            "",
            "Error: no tracker for this repository here yet: run `bead sync` to fetch it from its remote, or `bead init` to start one if it has none",
        ),
        (
            1,
            "",
            r#"{"error": {"code": "NOT_INITIALIZED", "message": "no tracker for this repository here yet: run `bead sync` to fetch it from its remote, or `bead init` to start one if it has none"}}"#,
        ),
    );
    ask(&m, None, "", &uninitialised);
    json(&m, &["init", "--prefix", "ex"]);
    let key = id(&json(&m, &["create", "seed"]));
    let before = m.export();
    for case in &ARGUMENTS {
        ask(&m, None, &key, case);
    }
    assert_eq!(m.export(), before, "nothing was written");
}

/// Expected `aliases.json` after renaming, with `<replica>` for this machine. Invalid bytes
/// remain lone surrogate escapes, and existing members are preserved.
const ALIASES: &str = r#"{
 "zz": "new",
 "a\udcff": "again",
 "n": {
  "x": [
   1,
   2.5,
   "\u00e9"
  ]
 },
 "dup": "last",
 "ab\udcff": "c\udcfed",
 "<replica>": "host\udcff"
}"#;

/// `machines --name` preserves invalid bytes as lone surrogates along with any already in
/// `aliases.json`. Human output substitutes `?` for each.
#[test]
fn aliases_preserve_bytes_that_are_not_utf8() {
    let seeded = r#"{"zz": "kept", "a\udcff": "lone \udcfe", "n": {"x": [1, 2.50, "\u00e9"]}, "dup": 1, "dup": "last"}"#;
    let sandbox = crate::sandbox();
    let m = Machine::new(&sandbox, "names");
    json(&m, &["init", "--prefix", "ex"]);
    std::fs::write(m.home().join("aliases.json"), seeded).unwrap();
    let replica = json(&m, &["info"])["machine"].as_str().unwrap().to_owned();
    let own = [replica.as_bytes(), b"=host\xff"].concat();
    for given in [&b"ab\xff=c\xfed"[..], b"zz=new", b"a\xff=again", &own] {
        let renamed = m
            .cmd([
                OsStr::new("machines"),
                OsStr::new("--name"),
                OsStr::from_bytes(given),
            ])
            .run();
        assert_eq!(renamed.returncode(), 0, "{}", renamed.stderr);
    }
    let file = std::fs::read_to_string(m.home().join("aliases.json")).unwrap();
    assert_eq!(file.replace(&replica, "<replica>"), ALIASES);
    let listed = json(&m, &["machines"]);
    assert_eq!(listed[0]["alias"], Json::from("host?"));
    let human = m.run(["machines"]);
    let line = human.stdout.replace(&replica, "<replica>");
    let shown = line.split("events").next().unwrap();
    assert_eq!(shown, "<replica>  host?               1 ");
}

/// Paths named in errors. Test `bad\xff.jsonl` separately because some systems cannot create it.
const PATHS: [Case; 3] = [
    (
        &[b"create", b"t", b"--description-file", b"missing//x/."],
        (1, "", "Error: No such file or directory: missing/x"),
        (
            1,
            "",
            r#"{"error": {"code": "IO", "message": "No such file or directory: missing/x"}}"#,
        ),
    ),
    (
        &[b"import", b"./bad.jsonl/"],
        (
            1,
            "",
            "Error: ./bad.jsonl/ is not valid JSONL: Expecting property name enclosed in double \
             quotes: line 1 column 2 (char 1)",
        ),
        (
            1,
            "",
            r#"{"error": {"code": "VALIDATION", "message": "./bad.jsonl/ is not valid JSONL: Expecting property name enclosed in double quotes: line 1 column 2 (char 1)"}}"#,
        ),
    ),
    (
        &[b"comments", b"add", b"{key}", b"-f", b"no//such/"],
        (1, "", "Error: No such file or directory: no/such"),
        (
            1,
            "",
            r#"{"error": {"code": "IO", "message": "No such file or directory: no/such"}}"#,
        ),
    ),
];

/// Command-line paths are normalised (`dump.jsonl/.` becomes `dump.jsonl`). Errors use the
/// normalised path or original argument according to the operation, in both output modes.
#[test]
fn paths_are_normalized_consistently_for_reads_and_writes() {
    let sandbox = crate::sandbox();
    let m = Machine::new(&sandbox, "paths");
    json(&m, &["init", "--prefix", "ex"]);
    let key = id(&json(&m, &["create", "seed"]));
    let dir = m.checkout();
    std::fs::write(dir.join("note.txt"), "from a file\n").unwrap();
    std::fs::write(dir.join("bad.jsonl"), "{not json\n").unwrap();
    // a name that is not UTF-8: Linux takes it; macOS refuses it, and the import fails to open it
    let odd = dir.join(OsStr::from_bytes(b"bad\xff.jsonl"));
    let made = std::fs::write(&odd, "{not json\n").is_ok();
    for target in ["dump.jsonl/", "dump.jsonl/.", "./sub//..//dump.jsonl"] {
        let _ = std::fs::remove_file(dir.join("dump.jsonl"));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let exported = answer(&m, None, &key, &[b"export", b"-o", target.as_bytes()]);
        let said = said(0, &format!("Exported 1 issues to {target}"), "", &key);
        assert_eq!(exported, said, "{target}");
        assert!(dir.join("dump.jsonl").is_file(), "{target}");
    }
    for case in &PATHS {
        ask(&m, None, &key, case);
    }
    let imported = answer(&m, None, &key, &[b"import", b"bad\xff.jsonl", b"--json"]);
    if made {
        // On systems accepting this filename, the file must be readable.
        assert_eq!(imported.0, 1, "{imported:?}");
        assert!(imported.2.contains("is not valid JSONL"), "{imported:?}");
    } else {
        let error =
            r#"{"error": {"code": "IO", "message": "No such file or directory: bad\udcff.jsonl"}}"#;
        assert_eq!(imported, said(1, "", error, &key));
    }
    let read = answer(
        &m,
        None,
        &key,
        &[b"comments", b"add", b"{key}", b"-f", b"note.txt/"],
    );
    assert_eq!(read, said(0, "Comment added to {key}", "", &key));
    let comments = json(&m, &["comments", &key]);
    assert_eq!(comments.as_array().unwrap().len(), 1, "{comments:?}");
}

/// An unavailable TMPDIR falls back to `/tmp` for git's captured output.
#[test]
fn a_tmpdir_that_does_not_exist_uses_a_writable_fallback() {
    let sandbox = crate::sandbox();
    let m = Machine::new(&sandbox, "tmp");
    let missing = sandbox.root().join("no-such-tmp");
    let init = m
        .cmd(["init", "--prefix", "ex", "--json"])
        .env("TMPDIR", &missing)
        .run();
    assert_eq!(init.returncode(), 0, "{}", init.stderr);
}
