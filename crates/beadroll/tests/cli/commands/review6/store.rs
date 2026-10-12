//! F10, F11, F12, F13 and F16: the store's files when they are not what bead wrote. An attempt
//! with a line that is not an event is inert, a stale pending twin of a confirmed attempt is no
//! fork, a junk pending file is swept instead of fenced on every sync, the local state files are
//! read as strict JSON, and a read-only store refuses a write without losing what it holds.

use std::path::{Path, PathBuf};
use std::process::Command;

use beadroll_test_support::{Machine, Run, Sandbox, code, git, parse_json};

use super::{glob, id, machine};

/// The attempt files in `machine`'s store, in directory order: `events.glob("*/claims/*.json")`.
fn attempts(machine: &Machine) -> Vec<PathBuf> {
    let Ok(replicas) = std::fs::read_dir(machine.store().join("events")) else {
        return Vec::new();
    };
    replicas
        .flat_map(|replica| glob(&replica.unwrap().path().join("claims"), ".json"))
        .collect()
}

/// Splits bytes at `\n`, `\r\n` or `\r`, without a final empty line.
fn splitlines(bytes: &[u8]) -> Vec<&[u8]> {
    let mut lines = Vec::new();
    let (mut start, mut at) = (0, 0);
    while at < bytes.len() {
        let end = match (bytes[at], bytes.get(at + 1)) {
            (b'\r', Some(b'\n')) => at + 2,
            (b'\n' | b'\r', _) => at + 1,
            _ => {
                at += 1;
                continue;
            }
        };
        lines.push(&bytes[start..at]);
        (start, at) = (end, end);
    }
    if start < bytes.len() {
        lines.push(&bytes[start..]);
    }
    lines
}

/// The commits on the remote's `refs/beads/journal`.
fn journal_commits(sandbox: &Sandbox) -> u64 {
    let origin = sandbox.origin().to_str().unwrap();
    let count = git(
        [
            "--git-dir",
            origin,
            "rev-list",
            "--count",
            "refs/beads/journal",
        ],
        None,
    );
    count.trim().parse().unwrap()
}

/// `chmod -R MODE PATH`, which must succeed.
fn chmod(mode: &str, path: &Path) {
    let status = Command::new("chmod")
        .args(["-R", mode])
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success(), "chmod -R {mode} {}", path.display());
}

/// `info --json` must print strict JSON.
fn assert_strict_json(info: &Run) {
    if let Err(error) = parse_json(&info.stdout) {
        let start: String = info.stdout.chars().take(240).collect();
        panic!("info --json was not strict JSON: {error}; {start}");
    }
}

/// F10: a later JSON value that is not an event voids the whole attempt.
#[test]
fn f10_non_event_json_makes_the_whole_attempt_inert() {
    let sandbox = crate::sandbox();
    let writer = machine(&sandbox, "f10");
    let key = id(writer.bead(["create", "attempt"]));
    writer.bead(["sync"]);
    let reader = Machine::new(&sandbox, "f10-reader");
    reader.bead(["sync"]);
    writer.bead([
        "update",
        &key,
        "--claim",
        "--title",
        "kept",
        "--add-label",
        "kept",
    ]);
    reader.bead(["sync"]);
    let attempt = attempts(&reader)
        .into_iter()
        .find(|path| splitlines(&std::fs::read(path).unwrap()).len() >= 2)
        .expect("an attempt of two lines or more");
    let bytes = std::fs::read(&attempt).unwrap();
    let mut tampered = splitlines(&bytes)[0].to_vec();
    tampered.extend_from_slice(b"\nnull\n");
    std::fs::write(&attempt, tampered).unwrap();
    let shown = reader.show(&key);
    assert!(shown.get("claim").is_none(), "{shown:?}");
    assert_eq!(shown["title"].as_str(), Some("attempt"));
    assert!(reader.bead(["info"]).unwrap()["integrity"].truthy());
}

/// F11: a leftover pending twin must not raise ATTEMPT_FORK when the confirmed file matches.
#[test]
fn f11_stale_pending_twin_does_not_fork_a_matching_attempt() {
    let sandbox = crate::sandbox();
    let writer = machine(&sandbox, "f11");
    let key = id(writer.bead(["create", "twin"]));
    writer.bead(["sync"]);
    let reader = Machine::new(&sandbox, "f11-reader");
    reader.bead(["sync"]);
    writer.bead(["claim", &key]);
    reader.bead(["sync"]);
    let attempt = attempts(&reader).into_iter().next().expect("an attempt");
    let pending = reader.store().join("pending");
    std::fs::create_dir_all(&pending).unwrap();
    let mut twin = std::fs::read(&attempt).unwrap();
    twin.push(b'\n');
    let stem = attempt.file_stem().unwrap().to_str().unwrap();
    std::fs::write(pending.join(format!("{stem}.json")), twin).unwrap();
    let synced = reader.run(["sync", "--json"]);
    assert_eq!(synced.returncode(), 0, "{}", synced.stderr);
    assert!(!synced.stderr.contains("ATTEMPT_FORK"), "{}", synced.stderr);
    let shown = reader.show(&key);
    assert!(shown.get("claim").is_some(), "{shown:?}");
}

/// F12: one corrupt pending file does not add a fence commit on every later sync.
#[test]
fn f12_unreadable_pending_file_is_not_fenced_forever() {
    let sandbox = crate::sandbox();
    let machine = machine(&sandbox, "f12");
    machine.bead(["create", "quiet"]);
    machine.bead(["sync"]);
    let pending = machine.store().join("pending");
    std::fs::create_dir_all(&pending).unwrap();
    let junk = pending.join("01900000-0000-7000-8000-0000000000aa.json");
    std::fs::write(&junk, "{").unwrap();
    let before = journal_commits(&sandbox);
    let first = machine.run(["sync", "--json"]);
    let second = machine.run(["sync", "--json"]);
    let after = journal_commits(&sandbox);
    assert_eq!(first.returncode(), 0, "{}", first.stderr);
    assert_eq!(second.returncode(), 0, "{}", second.stderr);
    assert!(!junk.exists());
    assert!(
        after <= before + 1,
        "{before} commits before, {after} after"
    );
}

/// F13: meta, replica and aliases reject non-strict or wrong-shaped JSON without a traceback.
#[test]
fn f13_local_state_files_are_strict_json() {
    let sandbox = crate::sandbox();
    let machine = machine(&sandbox, "f13");
    let meta = machine.store().join("meta.json");
    std::fs::write(meta, "{\"last_sync\": NaN, \"name\": \"ex\"}\n").unwrap();
    let info = machine.run(["info", "--json"]);
    assert_eq!(info.returncode(), 0, "{}", info.stderr);
    assert!(!info.crashed(), "{}", info.stderr);
    assert_strict_json(&info);

    let replica = machine.home().join("replica.json");
    let saved = std::fs::read(&replica).unwrap();
    std::fs::write(&replica, "{\"id\": NaN}\n").unwrap();
    let info = machine.run(["info", "--json"]);
    assert!(!info.crashed(), "{}", info.stderr);
    if info.returncode() == 0 {
        assert_strict_json(&info);
    } else {
        assert_eq!(code(&info), "IDENTITY", "{}", info.stderr);
    }
    let created = machine.run(["create", "after nan identity", "--json"]);
    assert!(!created.crashed(), "{}", created.stderr);
    std::fs::write(&replica, saved).unwrap();

    std::fs::write(machine.home().join("aliases.json"), "[1, 2]\n").unwrap();
    let listed = machine.run(["machines", "--json"]);
    assert!(!listed.crashed(), "{}", listed.stderr);
    assert_eq!(listed.returncode(), 0, "{}", listed.stderr);
}

/// F16: writing a read-only store fails without a traceback and keeps what was already there.
#[test]
fn f16_read_only_store_write_is_a_structured_error() {
    let sandbox = crate::sandbox();
    let machine = machine(&sandbox, "f16");
    let key = id(machine.bead(["create", "kept"]));
    let store = machine.store();
    chmod("a-w", &store);
    let created = machine.run(["create", "nope", "--json"]);
    assert_ne!(created.returncode(), 0, "{}", created.stdout);
    assert!(!created.crashed(), "{}", created.stderr);
    let shown = machine.run(["show", &key, "--json"]);
    assert_eq!(shown.returncode(), 0, "{}", shown.stderr);
    assert_eq!(
        parse_json(&shown.stdout).unwrap()[0]["title"].as_str(),
        Some("kept")
    );
    // The `finally`: should an assertion fail, the sandbox's drop makes the store writable again.
    chmod("u+w", &store);
}
