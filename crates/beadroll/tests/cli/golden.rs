//! Golden outputs, for what other tests do not pin whole (`docs/RUST-PORT.md` §4.2 item 5): every
//! help page, the whole human `show`, the human output of the other reading commands over one
//! scripted tracker, and `info`'s keys, with ids, keys, machines, commits, times and paths
//! normalised (`golden/normal.rs`). Each test compares the `bead` under test with its file in
//! `tests/golden/`. The files were captured from Python's `./bead` under CPython 3.14, the reference
//! interpreter (`docs/RUST-PORT.md` §5), and only from it, until the cutover removed the launcher
//! (`python-0.1.0` has it, and `cargo xtask golden`, which captured them): they are what bead
//! answered before the port, and regenerating one to make the Rust bead pass is never the fix. A
//! deliberate change of what bead prints edits the file by hand, in the change that makes it.

mod normal;

use std::path::{Path, PathBuf};

use beadroll_test_support::{Machine, Sandbox};

use normal::Normaliser;

/// One test's commands and what `bead` printed for them.
struct Transcript {
    name: &'static str,
    normal: Normaliser,
    text: String,
}

impl Transcript {
    fn new(name: &'static str, sandbox: &Sandbox) -> Transcript {
        let root = sandbox.root();
        let mut roots = vec![root.display().to_string()];
        if let Ok(real) = root.canonicalize() {
            roots.push(real.display().to_string());
        }
        Transcript {
            name,
            normal: Normaliser::new(roots),
            text: String::new(),
        }
    }

    /// Runs `bead ARGS` on `machine` and records it: the command, its standard output, its
    /// standard error when there is any, and its exit status.
    fn record(&mut self, machine: &Machine, args: &[&str]) {
        self.record_with(machine, args, &[]);
    }

    /// [`Transcript::record`] with variables set for the run.
    fn record_with(&mut self, machine: &Machine, args: &[&str], vars: &[(&str, &str)]) {
        let mut command = machine.cmd(args);
        for (key, value) in vars {
            command = command.env(*key, *value);
        }
        let run = command.run();
        let mut entry = format!("$ bead {}\n", args.join(" "));
        entry.push_str(&run.stdout);
        if !run.stderr.is_empty() {
            entry.push_str("stderr: ");
            entry.push_str(&run.stderr);
        }
        entry.push_str(&format!("exit {}\n\n", run.returncode()));
        let entry = self.normal.apply(&entry);
        self.text.push_str(&entry);
    }

    /// Records a line of the test's own.
    fn note(&mut self, line: &str) {
        let line = self.normal.apply(line);
        self.text.push_str(&format!("# {line}\n\n"));
    }

    /// Compares the transcript with its golden file.
    fn finish(self) {
        let path = golden_dir().join(format!("{}.txt", self.name));
        let expected =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        if self.text != expected {
            let first = self
                .text
                .lines()
                .zip(expected.lines())
                .position(|(a, b)| a != b)
                .unwrap_or_else(|| self.text.lines().count().min(expected.lines().count()));
            let show = |text: &str| -> String {
                text.lines()
                    .skip(first.saturating_sub(3))
                    .take(12)
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            panic!(
                "{} differs from Python's at line {}:\n--- golden (Python)\n{}\n--- this bead\n{}",
                path.display(),
                first + 1,
                show(&expected),
                show(&self.text)
            );
        }
    }
}

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
}

/// The tracker the reading commands are run over: Beads exports with fixed keys, then native work on
/// top of them. The epic comes first and alone, then its children, a bug, a closed feature and a
/// dependency on a key that is not an issue ([`ISSUES`]), then a cycle in two parts ([`LOOP_ONE`],
/// [`LOOP_TWO`]): what one import brings is minted within a millisecond, in an order no run
/// repeats, and `show` lists dependencies, `dep cycles` its cycles, by issue id.
const EPIC: &str = r#"{"id":"ex-a","title":"Ship the port","description":"Port bead to Rust.\nKeep parity.","acceptance_criteria":"Every verb ported","design":"One table drives the parser.","notes":"Phase 2b","status":"open","priority":1,"issue_type":"epic","assignee":"maintainer","estimated_minutes":90,"external_ref":"gh-12","labels":["port","rust"],"created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-02T00:00:00Z","created_by":"seed"}
"#;

/// The epic's children and the other issues, imported after it (see [`EPIC`]).
const ISSUES: &str = r#"{"id":"ex-a.1","title":"Parser","acceptance_criteria":"argparse parity","status":"open","priority":2,"issue_type":"task","created_at":"2026-01-01T01:00:00Z","updated_at":"2026-01-01T01:00:00Z","created_by":"seed","dependencies":[{"issue_id":"ex-a.1","depends_on_id":"ex-a","type":"parent-child","created_at":"2026-01-01T01:00:00Z","created_by":"seed"}],"comments":[{"id":7,"issue_id":"ex-a.1","author":"reviewer","text":"imported note","created_at":"2026-01-01T02:00:00Z"}]}
{"id":"ex-a.2","title":"Help pages","status":"open","priority":2,"issue_type":"task","created_at":"2026-01-01T02:00:00Z","updated_at":"2026-01-01T02:00:00Z","created_by":"seed","dependencies":[{"issue_id":"ex-a.2","depends_on_id":"ex-a","type":"parent-child","created_at":"2026-01-01T02:00:00Z","created_by":"seed"},{"issue_id":"ex-a.2","depends_on_id":"ex-a.1","type":"blocks","created_at":"2026-01-01T02:00:00Z","created_by":"seed"}]}
{"id":"ex-b","title":"Crash on start","description":"bead dies when stdout is closed","status":"open","priority":0,"issue_type":"bug","created_at":"2026-01-01T03:00:00Z","updated_at":"2026-01-01T03:00:00Z","created_by":"seed"}
{"id":"ex-c","title":"Old feature","status":"closed","priority":3,"issue_type":"feature","close_reason":"shipped","closed_at":"2026-01-03T00:00:00Z","created_at":"2026-01-01T04:00:00Z","updated_at":"2026-01-03T00:00:00Z","created_by":"seed"}
{"id":"ex-d","title":"Waits on a key","acceptance_criteria":"done","status":"open","priority":2,"issue_type":"task","created_at":"2026-01-01T05:00:00Z","updated_at":"2026-01-01T05:00:00Z","created_by":"seed","dependencies":[{"issue_id":"ex-d","depends_on_id":"ex-zzz","type":"blocks","created_at":"2026-01-01T05:00:00Z","created_by":"seed"}]}
"#;

/// The first half of a cycle: it waits on a key that is not an issue yet.
const LOOP_ONE: &str = r#"{"id":"ex-e","title":"Loop one","status":"open","priority":4,"issue_type":"chore","created_at":"2026-01-01T06:00:00Z","updated_at":"2026-01-01T06:00:00Z","created_by":"seed","dependencies":[{"issue_id":"ex-e","depends_on_id":"ex-f","type":"blocks","created_at":"2026-01-01T06:00:00Z","created_by":"seed"}]}
"#;

/// The second half: importing it closes the cycle.
const LOOP_TWO: &str = r#"{"id":"ex-f","title":"Loop two","status":"open","priority":4,"issue_type":"chore","created_at":"2026-01-01T07:00:00Z","updated_at":"2026-01-01T07:00:00Z","created_by":"seed","dependencies":[{"issue_id":"ex-f","depends_on_id":"ex-e","type":"blocks","created_at":"2026-01-01T07:00:00Z","created_by":"seed"}]}
"#;

#[test]
fn help_pages() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "gold-help");
    let mut t = Transcript::new("help", &sandbox);
    let wide = [("COLUMNS", "80")];
    t.record_with(&a, &[], &wide);
    t.record_with(&a, &["help"], &wide);
    t.record_with(&a, &["-h", "x"], &wide);
    for name in [
        "init", "import", "export", "create", "update", "claim", "release", "takeover", "claims",
        "close", "reopen", "delete", "show", "list", "ready", "blocked", "search", "dep",
        "comments", "lint", "sync", "log", "info", "machines", "help", "prime", "onboard",
    ] {
        t.record_with(&a, &[name, "--help"], &wide);
        t.record_with(&a, &["help", name], &wide);
    }
    for args in [
        &["create", "--help"][..],
        &["ready", "--help"],
        &["-h", "x"],
    ] {
        t.record_with(&a, args, &[("COLUMNS", "40")]);
    }
    t.record_with(&a, &["help", "nonsense"], &wide);
    t.record_with(&a, &["--json", "help", "nonsense"], &wide);
    t.record(&a, &["onboard"]);
    t.finish();
}

/// What `reading_commands_over_one_tracker` asks of the tracker, after building it: each issue
/// in full, then the other reading commands, and errors.
const READINGS: &[&[&str]] = &[
    &["list"],
    &["list", "--all"],
    &["list", "-s", "closed"],
    &["list", "--parent", "ex-a"],
    &["list", "-t", "task", "-p", "2"],
    &["ready"],
    &["ready", "ex-a"],
    &["ready", "--parent", "ex-a"],
    &["ready", "--limit", "1"],
    &["blocked"],
    &["search", "parser"],
    &["search", "-a", "old"],
    &["search", "nothing-like-it"],
    &["lint"],
    &["dep", "list", "ex-a.2"],
    &["dep", "ex-d"],
    &["dep", "tree", "ex-a"],
    &["dep", "cycles"],
    &["dep", "cycles", "--blocking-only"],
    &["comments", "ex-a.1"],
    &["comments", "ex-b"],
    &["log", "ex-a.1", "--limit", "0"],
    &["log", "--limit", "3"],
    &["machines"],
    &["show", "ex-nope"],
    &["dep", "add", "ex-a", "ex-a"],
    &["dep", "add", "ex-a.1", "ex-a.2"],
    &["update", "ex-b", "--status", "bogus"],
    &["update", "ex-b", "--status", "tombstone"],
    &["comments", "add", "ex-b", " "],
    &["create"],
    &["close", "ex-nope"],
];

#[test]
fn reading_commands_over_one_tracker() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "gold-a");
    let b = Machine::new(&sandbox, "gold-b");
    let mut t = Transcript::new("tracker", &sandbox);
    let loose = build_tracker(&mut t, &sandbox, &a);
    for key in [
        "ex-a", "ex-a.1", "ex-a.2", "ex-a.3", "ex-b", "ex-c", "ex-d", "ex-e", &loose,
    ] {
        t.record(&a, &["show", key]);
    }
    for args in READINGS {
        t.record(&a, args);
    }
    // two machines set one title without seeing each other: a conflict
    t.record(&b, &["sync"]);
    t.record(
        &a,
        &["update", "ex-d", "--title", "Title from A", "--offline"],
    );
    t.record(
        &b,
        &["update", "ex-d", "--title", "Title from B", "--offline"],
    );
    t.record(&a, &["sync"]);
    t.record(&b, &["sync"]);
    t.record(&a, &["sync"]);
    t.record(&a, &["show", "ex-d"]);
    t.record(&b, &["list", "--id", "ex-d"]);
    t.finish();
}

/// The scripted tracker on `a`: the imports, then native work with every command that writes;
/// the key of the issue it deletes.
fn build_tracker(t: &mut Transcript, sandbox: &Sandbox, a: &Machine) -> String {
    let epic = sandbox.root().join("epic.jsonl");
    std::fs::write(&epic, EPIC).expect("the export");
    t.record(
        a,
        &[
            "init",
            "--prefix",
            "ex",
            "--import",
            &epic.display().to_string(),
        ],
    );
    for (name, records) in [
        ("issues.jsonl", ISSUES),
        ("loop-one.jsonl", LOOP_ONE),
        ("loop-two.jsonl", LOOP_TWO),
    ] {
        let path = sandbox.root().join(name);
        std::fs::write(&path, records).expect("the export");
        t.record(a, &["import", &path.display().to_string()]);
    }
    // the machine's own display name is its host's: one that does not change from host to host
    let info = a.bead(["info"]).expect("info");
    let replica = info["machine"].as_str().expect("a machine id");
    t.record(a, &["machines", "--name", &format!("{replica}=gold-a")]);
    t.record(
        a,
        &[
            "create",
            "Native child",
            "--parent",
            "ex-a",
            "-p",
            "1",
            "-d",
            "first line\nsecond line",
            "--acceptance",
            "renders",
            "-l",
            "beta",
            "-l",
            "alpha",
        ],
    );
    let loose = a
        .bead(["create", "Loose end", "-t", "chore", "-p", "3", "--silent"])
        .and_then(|r| r["id"].as_str().map(str::to_owned))
        .expect("a key");
    t.note(&format!("created {loose}"));
    t.record(a, &["comments", "add", "ex-a.1", "native comment"]);
    t.record(a, &["comments", "add", "ex-a.1", "-m", "- dash item"]);
    t.record(a, &["dep", "add", "ex-b", "ex-a.1"]);
    t.record(
        a,
        &[
            "update",
            "ex-b",
            "--notes",
            "investigating",
            "--add-label",
            "urgent",
            "--assignee",
            "alice",
        ],
    );
    t.record(a, &["update", "ex-a.2", "--status", "closed"]);
    t.record(a, &["close", "ex-a.3", "-r", "done for now"]);
    t.record(a, &["reopen", "ex-a.3"]);
    t.record(a, &["delete", &loose, "-r", "not needed"]);
    loose
}

#[test]
fn info_keys() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "gold-info");
    a.bead(["init", "--prefix", "ex"]);
    let mut t = Transcript::new("info", &sandbox);
    let info = a.bead(["info"]).expect("info");
    let keys: Vec<&str> = info
        .as_object()
        .expect("info is an object")
        .keys()
        .map(String::as_str)
        .collect();
    t.note(&format!("--json info keys: {}", keys.join(" ")));
    let human = a.run(["info"]);
    let keys: Vec<&str> = human
        .stdout
        .lines()
        .filter_map(|line| line.split_once(':').map(|(key, _)| key))
        .collect();
    t.note(&format!("info line keys: {}", keys.join(" ")));
    t.finish();
}
