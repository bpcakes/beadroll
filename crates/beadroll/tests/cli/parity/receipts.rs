//! Receipt signatures are a byte-level format contract (`docs/REVIEWING.md` §1).
//! Equivalent command spellings produce identical receipts; an owner's close and delete leave
//! none. Fixed reference fixtures pin compatibility across versions; their provenance is in
//! `docs/REVIEWING.md` §2. Helpers also serve `parity::retries`.

use std::collections::BTreeSet;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use beadroll_test_support::{Json, Machine, Sandbox, parse_json};

/// The crash hook that stops a coordinated command once the remote has taken its attempt, before
/// it counts here: the attempt stays pending, its receipt with it.
pub(super) const CRASH_AFTER: (&str, &str) = ("BEAD_TEST_CRASH_AFTER_PUSH", "1");

/// The tracker every case runs on: Beads records with fixed keys, the same in every sandbox.
fn issues() -> String {
    ('a'..='l')
        .map(|c| {
            format!(
                "{{\"id\":\"ex-{c}\",\"title\":\"Issue {c}\",\"status\":\"open\",\"priority\":2,\
                 \"issue_type\":\"task\",\"created_at\":\"2026-01-01T00:00:00Z\",\
                 \"updated_at\":\"2026-01-01T00:00:00Z\",\"created_by\":\"seed\"}}\n"
            )
        })
        .collect()
}

/// `bead ARGS` run as `machine`, with `stdin` fed to it and `vars` set over the machine's
/// environment.
pub(super) fn call(machine: &Machine, args: &[&str], stdin: &str, vars: &[(&str, &str)]) -> Output {
    let mut child = Command::new(machine.executable())
        .args(args)
        .current_dir(machine.checkout())
        .env_clear()
        .envs(machine.env(None, "codex"))
        .envs(vars.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("bead starts");
    if let Some(mut input) = child.stdin.take() {
        // bead may not read it at all
        let _ = input.write_all(stdin.as_bytes());
    }
    child.wait_with_output().expect("bead's output")
}

/// What `call` printed on success; a failure panics with bead's standard error.
pub(super) fn ok(machine: &Machine, args: &[&str]) -> Json {
    let mut all = args.to_vec();
    all.push("--json");
    let out = call(machine, &all, "", &[]);
    assert!(
        out.status.success(),
        "{args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let printed = String::from_utf8_lossy(&out.stdout);
    if printed.trim().is_empty() {
        return Json::Null;
    }
    parse_json(&printed).expect("bead's JSON")
}

/// A fresh tracker on `machine`: the fixed issues, and a description file in its checkout.
pub(super) fn tracker(sandbox: &Sandbox, machine: &Machine) {
    let export = sandbox.root().join("issues.jsonl");
    std::fs::write(&export, issues()).expect("the export");
    let export = export.display().to_string();
    ok(machine, &["init", "--prefix", "ex", "--import", &export]);
    std::fs::write(machine.checkout().join("desc.txt"), "from a file\n").expect("the file");
}

/// The receipts in `machine`'s store.
pub(super) fn receipts(machine: &Machine) -> BTreeSet<PathBuf> {
    let Ok(entries) = std::fs::read_dir(machine.store().join("pending")) else {
        return BTreeSet::new();
    };
    entries
        .map(|entry| entry.expect("an entry").path())
        .filter(|path| path.extension().is_some_and(|e| e == "command"))
        .collect()
}

/// A command line, and the variables it runs with.
type Step = (
    &'static [&'static str],
    &'static [(&'static str, &'static str)],
);

/// A command's setup, original spelling used for the fixture and an equivalent spelling.
#[derive(Clone, Copy)]
struct Case {
    /// Run first, to the end.
    before: &'static [Step],
    given: &'static [&'static str],
    respelt: &'static [&'static str],
    /// What it reads from standard input.
    stdin: &'static str,
    /// Whether it is coordinated: it then stops at the crash hook (99) with a receipt written, and
    /// otherwise ends normally without one.
    coordinated: bool,
}

/// A coordinated command, given as these two command lines.
const fn given(given: &'static [&'static str], respelt: &'static [&'static str]) -> Case {
    Case {
        before: &[],
        given,
        respelt,
        stdin: "",
        coordinated: true,
    }
}

/// The same command line both times.
const fn both(args: &'static [&'static str]) -> Case {
    given(args, args)
}

const CASES: [Case; 12] = [
    // a claim, with its defaults; --json never reaches the receipt, given to either
    both(&["claim", "ex-a"]),
    given(&["claim", "ex-b", "--json"], &["claim", "ex-b"]),
    given(
        &["claim", "ex-c", "ex-d"],
        &["claim", "ex-c", "ex-d", "--json"],
    ),
    // a release with its reason, by the short flag and by an abbreviation of the long one
    given(
        &["release", "ex-a", "-r", "done for now"],
        &["release", "ex-a", "--reas", "done for now"],
    ),
    // a takeover of another thread's claim
    Case {
        before: &[(&["claim", "ex-e"], &[("BEAD_THREAD", "other-thread")])],
        ..given(
            &["takeover", "ex-e", "-r", "stalled two days"],
            &["takeover", "ex-e", "--reason", "stalled two days"],
        )
    },
    // a claiming update: its fields, a description read from a file, labels, and aliases
    given(
        &[
            "update",
            "ex-f",
            "--claim",
            "--title",
            "Renamed",
            "--description-file",
            "desc.txt",
            "--add-label",
            "x",
            "--add-label",
            "y,z",
            "-p",
            "1",
        ],
        &[
            "update",
            "ex-f",
            "--claim",
            "--title",
            "Renamed",
            "--description-file",
            "desc.txt",
            "--add-l",
            "x",
            "--add-label",
            "y,z",
            "--priority",
            "1",
        ],
    ),
    // a description read from standard input
    Case {
        stdin: "from stdin\n",
        ..both(&["update", "ex-g", "--claim", "--description-file", "-"])
    },
    // --status in_progress claims
    given(
        &["update", "ex-h", "--status", "in_progress"],
        &["update", "ex-h", "-s", "in_progress"],
    ),
    // the actor is named, not detected
    both(&["claim", "ex-i", "--actor", "reviewer"]),
    given(
        &["update", "ex-j", "--claim", "--acceptance", "works"],
        &[
            "update",
            "ex-j",
            "--claim",
            "--acceptance-criteria",
            "works",
        ],
    ),
    // an owner's close and delete are local writes: no receipt
    Case {
        before: &[(&["claim", "ex-k"], &[])],
        coordinated: false,
        ..both(&["close", "ex-k", "-r", "finished"])
    },
    Case {
        before: &[(&["claim", "ex-l"], &[])],
        coordinated: false,
        ..both(&["delete", "ex-l"])
    },
];

/// Expected receipt for each case, identical for `given` and `respelt`, using machine m's actor.
const EXPECTED_RECEIPTS: [Option<&str>; 12] = [
    // ["claim", "ex-a"]
    Some(
        r#"{"actor": {"agent": "codex", "thread": "m-thread-0001"}, "args": {"command": "claim", "ids": ["ex-a"], "offline": false, "remote": null}, "request": null}"#,
    ),
    // ["claim", "ex-b", "--json"]
    Some(
        r#"{"actor": {"agent": "codex", "thread": "m-thread-0001"}, "args": {"command": "claim", "ids": ["ex-b"], "offline": false, "remote": null}, "request": null}"#,
    ),
    // ["claim", "ex-c", "ex-d"]
    Some(
        r#"{"actor": {"agent": "codex", "thread": "m-thread-0001"}, "args": {"command": "claim", "ids": ["ex-c", "ex-d"], "offline": false, "remote": null}, "request": null}"#,
    ),
    // ["release", "ex-a", "-r", "done for now"]
    Some(
        r#"{"actor": {"agent": "codex", "thread": "m-thread-0001"}, "args": {"command": "release", "ids": ["ex-a"], "offline": false, "reason": "done for now", "remote": null}, "request": null}"#,
    ),
    // ["takeover", "ex-e", "-r", "stalled two days"]
    Some(
        r#"{"actor": {"agent": "codex", "thread": "m-thread-0001"}, "args": {"command": "takeover", "ids": ["ex-e"], "offline": false, "reason": "stalled two days", "remote": null}, "request": null}"#,
    ),
    // ["update", "ex-f", "--claim", "--title", "Renamed", "--description-file", "desc.txt",
    //  "--add-label", "x", "--add-label", "y,z", "-p", "1"]
    Some(
        r#"{"actor": {"agent": "codex", "thread": "m-thread-0001"}, "args": {"acceptance_criteria": null, "add_label": ["x", "y,z"], "assignee": null, "claim": true, "command": "update", "description": null, "description_file": "desc.txt", "design": null, "estimate": null, "external_ref": null, "force": false, "ids": ["ex-f"], "notes": null, "offline": false, "parent": null, "priority": "1", "remote": null, "remove_label": null, "set_labels": null, "status": null, "title": "Renamed", "type": null}, "request": {"fields": {"description": "from a file\n", "priority": 1, "title": "Renamed"}, "status": null}}"#,
    ),
    // ["update", "ex-g", "--claim", "--description-file", "-"]
    Some(
        r#"{"actor": {"agent": "codex", "thread": "m-thread-0001"}, "args": {"acceptance_criteria": null, "add_label": null, "assignee": null, "claim": true, "command": "update", "description": null, "description_file": "-", "design": null, "estimate": null, "external_ref": null, "force": false, "ids": ["ex-g"], "notes": null, "offline": false, "parent": null, "priority": null, "remote": null, "remove_label": null, "set_labels": null, "status": null, "title": null, "type": null}, "request": {"fields": {"description": "from stdin\n"}, "status": null}}"#,
    ),
    // ["update", "ex-h", "--status", "in_progress"]
    Some(
        r#"{"actor": {"agent": "codex", "thread": "m-thread-0001"}, "args": {"acceptance_criteria": null, "add_label": null, "assignee": null, "claim": false, "command": "update", "description": null, "description_file": null, "design": null, "estimate": null, "external_ref": null, "force": false, "ids": ["ex-h"], "notes": null, "offline": false, "parent": null, "priority": null, "remote": null, "remove_label": null, "set_labels": null, "status": "in_progress", "title": null, "type": null}, "request": {"fields": {}, "status": null}}"#,
    ),
    // ["claim", "ex-i", "--actor", "reviewer"]
    Some(
        r#"{"actor": {"agent": "codex", "name": "reviewer", "thread": "m-thread-0001"}, "args": {"command": "claim", "ids": ["ex-i"], "offline": false, "remote": null}, "request": null}"#,
    ),
    // ["update", "ex-j", "--claim", "--acceptance", "works"]
    Some(
        r#"{"actor": {"agent": "codex", "thread": "m-thread-0001"}, "args": {"acceptance_criteria": "works", "add_label": null, "assignee": null, "claim": true, "command": "update", "description": null, "description_file": null, "design": null, "estimate": null, "external_ref": null, "force": false, "ids": ["ex-j"], "notes": null, "offline": false, "parent": null, "priority": null, "remote": null, "remove_label": null, "set_labels": null, "status": null, "title": null, "type": null}, "request": {"fields": {"acceptance_criteria": "works"}, "status": null}}"#,
    ),
    // ["close", "ex-k", "-r", "finished"]
    None,
    // ["delete", "ex-l"]
    None,
];

/// Runs every case on a fresh tracker, each given as `spelling` picks it: the receipt it wrote, if
/// any.
fn run_cases(spelling: fn(&Case) -> &'static [&'static str]) -> Vec<Option<String>> {
    let sandbox = crate::sandbox();
    let m = Machine::new(&sandbox, "m");
    tracker(&sandbox, &m);
    let mut written = Vec::new();
    for case in &CASES {
        for (args, vars) in case.before {
            let out = call(&m, args, "", vars);
            assert!(
                out.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        let known = receipts(&m);
        let args = spelling(case);
        let out = call(&m, args, case.stdin, &[CRASH_AFTER]);
        let status = if case.coordinated { 99 } else { 0 };
        assert_eq!(
            out.status.code(),
            Some(status),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let new: Vec<PathBuf> = receipts(&m).difference(&known).cloned().collect();
        written.push(match new.as_slice() {
            [] => None,
            [one] => Some(std::fs::read_to_string(one).expect("a receipt")),
            more => panic!("{args:?} wrote {} receipts", more.len()),
        });
    }
    written
}

#[test]
fn receipts_match_recorded_bytes_for_equivalent_command_spellings() {
    let spellings: [fn(&Case) -> &'static [&'static str]; 2] =
        [|case| case.given, |case| case.respelt];
    for spelling in spellings {
        let written = run_cases(spelling);
        for ((case, receipt), expected) in CASES.iter().zip(&written).zip(EXPECTED_RECEIPTS) {
            let args = spelling(case);
            assert_eq!(expected.is_some(), case.coordinated, "{args:?}");
            assert_eq!(receipt.as_deref(), expected, "{args:?}");
        }
    }
}
