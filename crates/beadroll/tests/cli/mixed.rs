//! Mixed fleets (`docs/RUST-PORT.md` §4.2, item 4): Python and Rust machines on one remote, and one
//! store written by one implementation and read or settled by the other. Each machine names its
//! binary, whatever `BEAD_UNDER_TEST` says: the `bead` this package builds, and the checkout's
//! Python launcher.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;

use beadroll_core::json::{Format, Object, Value, write};
use beadroll_core::replay::replay;
use beadroll_store::{
    Settings, Workspace, find_workspace, load_journal, write_atomic, write_attempt_file,
};
use beadroll_sync::{Link, PushOutcome, Tx, locked};
use beadroll_test_support::{Json, Machine, git, parse_json};

/// The Rust `bead` this package builds.
fn rust() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_bead"))
}

/// Python's launcher in this checkout (`./bead`), until the cutover removes it.
fn python() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../bead")
        .canonicalize()
        .expect("the checkout's Python launcher")
}

/// `bead ARGS` run by `bead` (either implementation) as `machine`: its checkout, home and identity.
fn run_as(bead: &Path, machine: &Machine, args: &[&str]) -> Output {
    Command::new(bead)
        .args(args)
        .current_dir(machine.checkout())
        .env_clear()
        .envs(machine.env(None, "codex"))
        .output()
        .expect("bead starts")
}

/// The JSON `bead ARGS --json` prints when run by `bead` as `machine`; a failure panics.
fn json_as(bead: &Path, machine: &Machine, args: &[&str]) -> Json {
    let mut all = args.to_vec();
    all.push("--json");
    let out = run_as(bead, machine, &all);
    assert!(
        out.status.success(),
        "{} {args:?} failed: {}",
        bead.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    parse_json(&String::from_utf8_lossy(&out.stdout)).unwrap()
}

fn id(value: &Json) -> String {
    value["id"].as_str().unwrap().to_owned()
}

/// Every issue a machine lists, in full, as sorted JSON: what two machines must agree on.
fn everything(bead: &Path, machine: &Machine) -> String {
    let listed = json_as(bead, machine, &["list", "--all", "--limit", "0"]);
    let mut args = vec!["show".to_owned()];
    args.extend(listed["issues"].as_array().unwrap().iter().map(id));
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    write(&json_as(bead, machine, &args), Format::SORTED_ASCII).unwrap()
}

/// The settings of `machine`'s runs, for reading or writing its store in-process.
fn settings(machine: &Machine) -> Arc<Settings> {
    let vars: BTreeMap<OsString, OsString> = machine.env(None, "codex").into_iter().collect();
    let checkout = machine.checkout().to_path_buf();
    let mut settings = Settings::from_process(
        vars.clone(),
        Some(checkout.clone()),
        std::env::var_os("HOME").unwrap_or_default(),
        std::env::temp_dir(),
    )
    .unwrap();
    settings.git = beadroll_git::Env::new(vars, Some(checkout), std::env::temp_dir());
    Arc::new(settings)
}

/// The integrity rows Rust's replay finds in `machine`'s store.
fn rust_integrity(machine: &Machine) -> usize {
    let ws = find_workspace(&settings(machine)).unwrap();
    let st = replay(load_journal(&ws).unwrap()).unwrap();
    st.integrity().len()
}

/// A Python and a Rust machine work on one tracker through one remote and converge: the same
/// issues in full on both, whichever binary reads either store, and neither replay reports an
/// integrity problem in a store the other wrote.
#[test]
fn python_and_rust_machines_converge() {
    let sandbox = crate::sandbox();
    let (py, rs) = (python(), rust());
    let p = Machine::new(&sandbox, "py");
    let r = Machine::new(&sandbox, "rs");
    json_as(&py, &p, &["init", "--prefix", "ex"]);
    let x = id(&json_as(
        &py,
        &p,
        &["create", "from python", "-p", "1", "-l", "a,b"],
    ));
    json_as(&rs, &r, &["sync"]);
    let y = id(&json_as(&rs, &r, &["create", "from rust", "--parent", &x]));
    let deps = format!("blocks:{y}");
    json_as(
        &rs,
        &r,
        &["create", "rust two", "--parent", &x, "--deps", &deps],
    );
    json_as(&py, &p, &["sync"]);
    json_as(
        &py,
        &p,
        &[
            "create",
            "python two",
            "--parent",
            &y,
            "-d",
            "née\u{2028}line",
        ],
    );
    for (bead, machine) in [(&py, &p), (&rs, &r), (&py, &p), (&rs, &r)] {
        json_as(bead, machine, &["sync"]);
    }
    let seen = everything(&py, &p);
    assert_eq!(everything(&rs, &r), seen, "the machines did not converge");
    assert_eq!(
        everything(&rs, &p),
        seen,
        "Rust reads Python's store differently"
    );
    assert_eq!(
        everything(&py, &r),
        seen,
        "Python reads Rust's store differently"
    );
    let info = json_as(&py, &r, &["info"]);
    assert_eq!(info["integrity"], Json::Array(Vec::new()), "{info:?}");
    assert_eq!(info["pending_ownership"], Json::Array(Vec::new()));
    assert_eq!((rust_integrity(&p), rust_integrity(&r)), (0, 0));
    assert_eq!(
        json_as(&rs, &r, &["ready"]),
        json_as(&py, &p, &["ready"]),
        "the machines disagree on readiness"
    );
}

/// The tree Rust publishes is the one Python publishes for the same store, byte for byte: the
/// manifest, every segment and its folders.
#[test]
fn the_published_tree_is_the_one_python_publishes() {
    let sandbox = crate::sandbox();
    let rs = rust();
    let r = Machine::new(&sandbox, "rs");
    json_as(&rs, &r, &["init", "--prefix", "ex"]);
    let x = id(&json_as(&rs, &r, &["create", "one", "-l", "x,y"]));
    json_as(&rs, &r, &["create", "two", "--parent", &x]);
    json_as(&rs, &r, &["sync"]);
    let journal_tree = |remote: &Path| {
        let git_dir = remote.as_os_str().to_owned();
        let args: [&OsStr; 4] = [
            "--git-dir".as_ref(),
            &git_dir,
            "rev-parse".as_ref(),
            "refs/beads/journal^{tree}".as_ref(),
        ];
        git(args, None).trim().to_owned()
    };
    let rust_tree = journal_tree(sandbox.origin());
    // Python publishes a copy of the same store to a remote of its own.
    let copy = sandbox.root().join("home-copy");
    let status = Command::new("cp")
        .arg("-R")
        .arg(r.home())
        .arg(&copy)
        .status()
        .unwrap();
    assert!(status.success());
    let other = sandbox.bare("other.git");
    let published = Command::new(python())
        .args(["sync", "--json", "--remote"])
        .arg(&other)
        .current_dir(r.checkout())
        .env_clear()
        .envs(r.env(None, "codex"))
        .env("BEAD_HOME", &copy)
        .output()
        .unwrap();
    assert!(
        published.status.success(),
        "{}",
        String::from_utf8_lossy(&published.stderr)
    );
    assert_eq!(journal_tree(&other), rust_tree);
    let manifest = git(
        [
            OsStr::new("--git-dir"),
            sandbox.origin().as_os_str(),
            OsStr::new("cat-file"),
            OsStr::new("blob"),
            OsStr::new(&format!("{rust_tree}:manifest.json")),
        ],
        None,
    );
    let workspace = r.store();
    let workspace = workspace.file_name().unwrap().to_str().unwrap();
    assert_eq!(
        manifest,
        format!("{{\"format\": 1, \"workspace\": \"{workspace}\"}}\n")
    );
}

/// What settling left of a store's pending attempts: the files in `pending/` by suffix, and whether
/// the issue is claimed.
fn settled(bead: &Path, machine: &Machine, key: &str) -> (usize, usize, bool) {
    let pending = machine.store().join("pending");
    let names: Vec<String> = std::fs::read_dir(&pending)
        .map(|entries| {
            entries
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    let count = |suffix: &str| names.iter().filter(|n| n.ends_with(suffix)).count();
    let shown = json_as(bead, machine, &["show", key]);
    (
        count(".json"),
        count(".command"),
        shown[0].get("claim").is_some(),
    )
}

/// A pending attempt and its receipt that Python left (accepted with its acknowledgement lost, or
/// never pushed) are settled by Rust's sync as Python's sync settles them: confirmed, or fenced and
/// dropped; the pending file goes, an accepted attempt's receipt stays.
#[test]
fn pending_attempts_python_left_are_settled_by_rust_as_by_python() {
    let py = python();
    let cases = [
        ("BEAD_TEST_CRASH_AFTER_PUSH", 99, (0, 1, true)),
        ("BEAD_TEST_CRASH_BEFORE_PUSH", 98, (0, 0, false)),
    ];
    for (hook, status, expected) in cases {
        for settler in [python(), rust()] {
            let sandbox = crate::sandbox();
            let p = Machine::new(&sandbox, "py");
            json_as(&py, &p, &["init", "--prefix", "ex"]);
            let x = id(&json_as(&py, &p, &["create", "x"]));
            json_as(&py, &p, &["sync"]);
            let crashed = Command::new(&py)
                .args(["claim", &x])
                .current_dir(p.checkout())
                .env_clear()
                .envs(p.env(None, "codex"))
                .env(hook, "1")
                .output()
                .unwrap();
            assert_eq!(crashed.status.code(), Some(status), "{hook}");
            assert_eq!(
                settled(&py, &p, &x),
                (1, 1, false),
                "{hook}: before settling"
            );
            json_as(&settler, &p, &["sync"]);
            assert_eq!(
                settled(&py, &p, &x),
                expected,
                "{hook}, settled by {}",
                settler.display()
            );
        }
    }
}

/// A claim of `issue` by `ws`'s actor written as an ownership command writes it, in-process: the
/// receipt first, then the pending attempt; with `publish`, pushed with the journal, its answer
/// lost (the attempt stays pending).
fn leave_pending(ws: &Workspace, actor: &Value, issue: &str, publish: bool) {
    let remote = ws.remote(None).unwrap().unwrap();
    let mut link = Link::new(ws, &remote).unwrap();
    locked(ws, || -> Result<(), beadroll_sync::Error> {
        let tip = link.fetch()?;
        link.merge(tip.as_deref())?;
        let mut tx = Tx::new(ws, actor.clone())?;
        let mut data = Object::new();
        data.insert("mode".into(), Value::from("online"));
        data.insert("generation".into(), Value::from(1));
        tx.emit("claim", Some(issue), data, None)?;
        let first = tx.ownership[0]["id"].as_str().unwrap().to_owned();
        let folder = ws.dir.join("pending");
        let receipt = format!(
            "{{\"actor\": {}, \"args\": {{\"command\": \"claim\"}}, \"request\": null}}",
            write(actor, Format::SORTED_ASCII).unwrap()
        );
        write_atomic(
            &folder.join(format!("{first}.command")),
            receipt.as_bytes(),
            true,
        )?;
        write_attempt_file(&folder.join(format!("{first}.json")), &tx.ownership)?;
        if publish {
            let outcome = link.publish(tip.as_deref(), "claim", &tx.ownership, false)?;
            assert_eq!(outcome, PushOutcome::Ok);
        }
        Ok(())
    })
    .unwrap();
}

/// The reverse: a pending attempt and receipt written by Rust (as the ownership protocol of Phase
/// 2c writes them: receipt, then attempt, then the push whose answer is lost) are settled by
/// Python's sync, and by Rust's, the same way.
#[test]
fn pending_attempts_rust_left_are_settled_by_python_as_by_rust() {
    for (publish, expected) in [(true, (0, 1, true)), (false, (0, 0, false))] {
        for settler in [python(), rust()] {
            let sandbox = crate::sandbox();
            let rs = rust();
            let r = Machine::new(&sandbox, "rs");
            json_as(&rs, &r, &["init", "--prefix", "ex"]);
            let x = id(&json_as(&rs, &r, &["create", "x"]));
            json_as(&rs, &r, &["sync"]);
            let settings = settings(&r);
            let ws = find_workspace(&settings).unwrap();
            let st = replay(load_journal(&ws).unwrap()).unwrap();
            let uid = st.resolve(&x).unwrap().uid().to_string();
            let actor =
                beadroll_test_support::json!({"agent": "codex", "thread": "rs-thread-0001"});
            leave_pending(&ws, &actor, &uid, publish);
            assert_eq!(settled(&rs, &r, &x), (1, 1, false), "before settling");
            json_as(&settler, &r, &["sync"]);
            assert_eq!(
                settled(&rs, &r, &x),
                expected,
                "published={publish}, settled by {}",
                settler.display()
            );
        }
    }
}
