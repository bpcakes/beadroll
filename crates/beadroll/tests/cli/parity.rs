//! What the mixed-fleet suite held the Rust bead to (`docs/RUST-PORT.md` §4.2, item 4), kept once
//! the cutover removed the Python launcher it compared the Rust bead with. Until then Python and
//! Rust machines shared one remote and one store, and every answer had to be Python's. Each test
//! now runs the Rust bead alone and keeps what holds without Python: machines converge, the
//! published tree is format 1's, sync settles pending attempts and keeps accepted receipts, a retry
//! recognises its receipt, and hostile input never crashes bead or writes anything. Where the test
//! was Python's answer itself, that answer is pinned here, as captured from Python 3.14's bead
//! before the cutover (`python-0.1.0`): the receipts' bytes, the unpublished counts, and what
//! bead says to arguments, actors and paths it reads as Python did. The help pages and the reading
//! commands are the goldens' (`golden.rs`).

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::process::Command;
use std::sync::Arc;

use beadroll_core::json::{Format, write};
use beadroll_core::replay::replay;
use beadroll_store::{Settings, find_workspace, load_journal};
use beadroll_test_support::{Json, Machine, git};

mod actors;
mod inputs;
mod pending;
mod receipts;
mod retries;

/// `value["id"]`, a string.
fn id(value: &Json) -> String {
    value["id"].as_str().expect("an id").to_owned()
}

/// `bead ARGS --json` on `machine`: the document it printed; a failure panics.
fn json(machine: &Machine, args: &[&str]) -> Json {
    machine.bead(args).unwrap_or(Json::Null)
}

/// Every issue a machine lists, in full, as sorted JSON: what two machines must agree on.
fn everything(machine: &Machine) -> String {
    let listed = json(machine, &["list", "--all", "--limit", "0"]);
    let mut args = vec!["show".to_owned()];
    args.extend(listed["issues"].as_array().expect("issues").iter().map(id));
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    write(&json(machine, &args), Format::SORTED_ASCII).expect("bead's JSON writes back")
}

/// The settings of `machine`'s runs, for reading or writing its store in-process.
fn settings(machine: &Machine) -> Arc<Settings> {
    let vars: BTreeMap<OsString, OsString> = machine.env(None, "codex").into_iter().collect();
    Arc::new(
        Settings::from_process(
            vars,
            Some(machine.checkout().to_path_buf()),
            std::env::var_os("HOME").unwrap_or_default(),
            std::env::temp_dir(),
            &|_| None,
        )
        .expect("the machine's settings"),
    )
}

/// The integrity rows the replay finds in `machine`'s store, read in this process.
fn integrity(machine: &Machine) -> usize {
    let ws = find_workspace(&settings(machine)).expect("the machine's workspace");
    let st = replay(load_journal(&ws).expect("its journal")).expect("its replay");
    st.integrity().len()
}

/// Two machines work on one tracker through one remote and converge: the same issues in full on
/// both, the same ready set, and no integrity problem in either store, read by `bead info` or in
/// this process. Until the cutover one machine ran Python's bead, and each implementation read the
/// other's store as its own.
#[test]
fn two_machines_converge() {
    let sandbox = crate::sandbox();
    let p = crate::rust_only(&sandbox, "p");
    let r = crate::rust_only(&sandbox, "r");
    json(&p, &["init", "--prefix", "ex"]);
    let x = id(&json(&p, &["create", "from p", "-p", "1", "-l", "a,b"]));
    json(&r, &["sync"]);
    let y = id(&json(&r, &["create", "from r", "--parent", &x]));
    let deps = format!("blocks:{y}");
    json(&r, &["create", "r two", "--parent", &x, "--deps", &deps]);
    json(&p, &["sync"]);
    let description = "née\u{2028}line";
    json(&p, &["create", "p two", "--parent", &y, "-d", description]);
    for machine in [&p, &r, &p, &r] {
        json(machine, &["sync"]);
    }
    let seen = everything(&p);
    assert_eq!(everything(&r), seen, "the machines did not converge");
    assert!(seen.contains("p two") && seen.contains("r two"), "{seen}");
    for machine in [&p, &r] {
        let info = json(machine, &["info"]);
        assert_eq!(info["integrity"], Json::Array(Vec::new()), "{info:?}");
        assert_eq!(info["pending_ownership"], Json::Array(Vec::new()));
    }
    assert_eq!((integrity(&p), integrity(&r)), (0, 0));
    assert_eq!(
        json(&r, &["ready"]),
        json(&p, &["ready"]),
        "the machines disagree on readiness"
    );
}

/// `git --git-dir REMOTE ARGS` in the sandbox: its output.
fn on_remote(remote: &std::path::Path, args: &[&str]) -> String {
    let git_dir: [&OsStr; 2] = ["--git-dir".as_ref(), remote.as_os_str()];
    git(git_dir.into_iter().chain(args.iter().map(OsStr::new)), None)
}

/// The tree published on `refs/beads/journal` is the one format 1 names (§1.1), the one Python's
/// bead published for the same store, as the mixed fleet checked byte for byte until the cutover:
/// `manifest.json`, `{"format": 1, "workspace": ID}` with sorted keys, spaced separators and a
/// newline, and a byte copy of each segment of each machine's journal.
#[test]
fn the_published_tree_is_format_1s() {
    let sandbox = crate::sandbox();
    let r = Machine::new(&sandbox, "r");
    json(&r, &["init", "--prefix", "ex"]);
    let x = id(&json(&r, &["create", "one", "-l", "x,y"]));
    json(&r, &["create", "two", "--parent", &x]);
    json(&r, &["sync"]);
    let origin = sandbox.origin();
    let tip = "refs/beads/journal^{tree}";
    let tree = on_remote(origin, &["rev-parse", tip]);
    let tree = tree.trim();
    let listed = on_remote(origin, &["ls-tree", "-r", "--name-only", tree]);
    let store = r.store();
    let replica = std::fs::read_dir(store.join("events"))
        .expect("the store's journals")
        .map(|entry| entry.expect("a journal").file_name())
        .next()
        .expect("this machine's journal");
    let replica = replica.to_str().expect("a replica id");
    let segment = format!("events/{replica}/000001.jsonl");
    assert_eq!(listed, format!("{segment}\nmanifest.json\n"));
    let blob = |path: &str| on_remote(origin, &["cat-file", "blob", &format!("{tree}:{path}")]);
    let workspace = store.file_name().expect("a store").to_str().expect("an id");
    assert_eq!(
        blob("manifest.json"),
        format!("{{\"format\": 1, \"workspace\": \"{workspace}\"}}\n")
    );
    let local = std::fs::read_to_string(store.join(&segment)).expect("the segment");
    assert_eq!(blob(&segment), local, "the published segment is not a copy");
}

/// A Beads export with what an import keeps: unknown fields, comments, a closed and a deleted
/// record, a dependency on a key that is not an issue yet.
const BEADS: &str = r#"{"id":"ex-m","title":"Imported epic","description":"from br","status":"open","priority":1,"issue_type":"epic","created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z","created_by":"seed","labels":["kept"],"source_repo":".","future_field":{"kept":true}}
{"id":"ex-m.1","title":"Imported child","status":"closed","priority":2,"issue_type":"task","created_at":"2026-01-01T01:00:00Z","updated_at":"2026-01-02T00:00:00Z","closed_at":"2026-01-02T00:00:00Z","close_reason":"done","created_by":"seed","external_ref":"gh-7","dependencies":[{"issue_id":"ex-m.1","depends_on_id":"ex-m","type":"parent-child","created_at":"2026-01-01T01:00:00Z","created_by":"seed","metadata":"{}","thread_id":""},{"issue_id":"ex-m.1","depends_on_id":"ex-later","type":"blocks","created_at":"2026-01-01T01:00:00Z","created_by":"seed"}],"comments":[{"id":41,"issue_id":"ex-m.1","author":"someone","text":"imported comment","created_at":"2026-01-01T02:00:00Z"}]}
{"id":"ex-gone","title":"Removed","status":"tombstone","priority":3,"issue_type":"bug","created_at":"2026-01-01T03:00:00Z","updated_at":"2026-01-03T00:00:00Z","deleted_at":"2026-01-03T00:00:00Z","deleted_by":"seed","delete_reason":"duplicate","created_by":"seed"}
"#;

/// `bead export` on `machine`: what it printed; a failure panics.
fn export(machine: &Machine) -> Vec<u8> {
    let out = Command::new(machine.executable())
        .arg("export")
        .current_dir(machine.checkout())
        .env_clear()
        .envs(machine.env(None, "codex"))
        .output()
        .expect("bead starts");
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{said}");
    out.stdout
}

/// Every command that writes (imports, creates, updates, labels, dependencies added and taken
/// back, comments, close, reopen, delete) on two machines: both export the tracker byte for byte
/// the same, to standard output or to a file, and show the same issues. Until the cutover one
/// machine ran Python's bead, and either implementation exported the other's store the same.
#[test]
fn every_write_exports_the_same_on_both_machines() {
    let sandbox = crate::sandbox();
    let (p, r) = (Machine::new(&sandbox, "p"), Machine::new(&sandbox, "r"));
    let beads = sandbox.root().join("beads.jsonl");
    std::fs::write(&beads, BEADS).unwrap();
    let beads = beads.display().to_string();
    json(&p, &["init", "--prefix", "ex", "--import", &beads]);
    let x = id(&json(
        &p,
        &["create", "p task", "--parent", "ex-m", "-l", "a,b"],
    ));
    json(&p, &["comments", "add", &x, "from p"]);
    json(&r, &["sync"]);
    let y = id(&json(&r, &["create", "r task", "-p", "0", "--deps", &x]));
    let notes = "née\u{2028}line";
    json(
        &r,
        &[
            "update",
            &y,
            "--notes",
            notes,
            "--add-label",
            "r",
            "--estimate",
            "45",
        ],
    );
    json(&r, &["update", &x, "--set-labels", "c", "--parent", &y]);
    let comment = [
        "comments", "add", &y, "-m", "from r", "--author", "reviewer",
    ];
    json(&r, &comment);
    json(&r, &["dep", "add", "ex-m", &y]);
    json(&r, &["dep", "remove", "ex-m", &y]);
    json(&r, &["dep", "rm", "ex-m.1", "ex-later"]);
    json(&r, &["close", &x, "-r", "done in r"]);
    json(&r, &["reopen", &x]);
    let z = id(&json(&r, &["create", "doomed"]));
    json(&r, &["delete", &z, "-r", "not needed"]);
    let more = sandbox.root().join("more.jsonl");
    let later = r#"{"id":"ex-later","title":"Arrived later","status":"open","priority":2,"issue_type":"task","created_at":"2026-01-04T00:00:00Z","updated_at":"2026-01-04T00:00:00Z","created_by":"seed"}"#;
    std::fs::write(&more, later).unwrap();
    json(&r, &["import", &more.display().to_string()]);
    for machine in [&r, &p, &r] {
        json(machine, &["sync"]);
    }
    let expected = export(&p);
    assert!(!expected.is_empty());
    assert_eq!(export(&r), expected, "the machines did not converge");
    let file = r.checkout().join("out.jsonl");
    json(&r, &["export", "-o", &file.display().to_string()]);
    assert_eq!(std::fs::read(&file).unwrap(), expected);
    assert_eq!(everything(&r), everything(&p));
}
