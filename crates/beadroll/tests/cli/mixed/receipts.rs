//! Receipts (`docs/RUST-PORT.md` §2.3j, §4.2 item 4, §5 row 21): a coordinated command leaves the
//! receipt Python's `command_signature` writes, byte for byte, whichever implementation runs it and
//! however its command line is spelt, and an owner's close and delete leave none. The helpers here
//! also serve `mixed::retries`, where each implementation recognises a receipt the other left.

use std::collections::BTreeSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use beadroll_test_support::{Json, Machine, Sandbox, parse_json};

use super::{python, rust};

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

/// `bead ARGS` run by `bead` as `machine`, with `stdin` fed to it and `vars` set over the
/// machine's environment.
pub(super) fn call(
    bead: &Path,
    machine: &Machine,
    args: &[&str],
    stdin: &str,
    vars: &[(&str, &str)],
) -> Output {
    let mut child = Command::new(bead)
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
pub(super) fn ok(bead: &Path, machine: &Machine, args: &[&str]) -> Json {
    let mut all = args.to_vec();
    all.push("--json");
    let out = call(bead, machine, &all, "", &[]);
    assert!(
        out.status.success(),
        "{} {args:?} failed: {}",
        bead.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    let printed = String::from_utf8_lossy(&out.stdout);
    if printed.trim().is_empty() {
        return Json::Null;
    }
    parse_json(&printed).expect("bead's JSON")
}

/// A fresh tracker on `machine`, by `bead`: the fixed issues, and a description file in its
/// checkout.
pub(super) fn tracker(bead: &Path, sandbox: &Sandbox, machine: &Machine) {
    let export = sandbox.root().join("issues.jsonl");
    std::fs::write(&export, issues()).expect("the export");
    let export = export.display().to_string();
    ok(
        bead,
        machine,
        &["init", "--prefix", "ex", "--import", &export],
    );
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

/// One command of the differential: what runs before it, then how Python and how Rust are given
/// it.
#[derive(Clone, Copy)]
struct Case {
    /// Run first, to the end.
    before: &'static [Step],
    python: &'static [&'static str],
    rust: &'static [&'static str],
    /// What it reads from standard input.
    stdin: &'static str,
    /// Whether it is coordinated: it then stops at the crash hook (99) with a receipt written, and
    /// otherwise ends normally without one.
    coordinated: bool,
}

/// A coordinated command, given to Python and to Rust as these command lines.
const fn given(python: &'static [&'static str], rust: &'static [&'static str]) -> Case {
    Case {
        before: &[],
        python,
        rust,
        stdin: "",
        coordinated: true,
    }
}

/// The same command line to both.
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

/// Runs every case with `bead` on a fresh tracker, each given as `given` picks it: the receipt it
/// wrote, if any.
fn run_cases(bead: &Path, given: fn(&Case) -> &'static [&'static str]) -> Vec<Option<String>> {
    let sandbox = crate::sandbox();
    let m = Machine::new(&sandbox, "m");
    tracker(bead, &sandbox, &m);
    let mut written = Vec::new();
    for case in &CASES {
        for (args, vars) in case.before {
            let out = call(bead, &m, args, "", vars);
            assert!(
                out.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        let known = receipts(&m);
        let args = given(case);
        let out = call(bead, &m, args, case.stdin, &[CRASH_AFTER]);
        let status = if case.coordinated { 99 } else { 0 };
        assert_eq!(
            out.status.code(),
            Some(status),
            "{} {args:?}: {}",
            bead.display(),
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
fn receipts_are_pythons_byte_for_byte_whoever_writes_them() {
    let from_python = run_cases(&python(), |case| case.python);
    let from_rust = run_cases(&rust(), |case| case.rust);
    for ((case, py), rs) in CASES.iter().zip(&from_python).zip(&from_rust) {
        assert_eq!(py.is_some(), case.coordinated, "{:?}", case.python);
        assert_eq!(py, rs, "{:?} and {:?} differ", case.python, case.rust);
    }
    assert_eq!(
        from_rust[0].as_deref(),
        Some(
            "{\"actor\": {\"agent\": \"codex\", \"thread\": \"m-thread-0001\"}, \"args\": \
             {\"command\": \"claim\", \"ids\": [\"ex-a\"], \"offline\": false, \"remote\": null}, \
             \"request\": null}"
        )
    );
}
