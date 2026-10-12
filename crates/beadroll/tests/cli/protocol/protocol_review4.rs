//! `ProtocolReview4`: the fourth independent review's ownership findings (2026-10-07-4-codex),
//! folded in after the fixes, at the command line. An attempt damaged in one store never replaces
//! the one the remote accepted, and an attempt with a malformed line is inert as a whole (Q01); the
//! retry of an accepted claiming update repeats none of its effects (Q02, on the crash hook
//! `BEAD_TEST_CRASH_AFTER_PUSH`, which both binaries implement: exit status 99 once the remote has
//! taken the push); a manifest of a future format at the remote is refused, never rewritten (Q04).
//! Q03 interrupts a release between its confirmation and its cleanup, which takes an in-crate API
//! test: beadroll-sync's `protocol::protocol_review4`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use beadroll_core::json::{Format, Style, write};
use beadroll_test_support::{Json, Machine, git, json, parse_json};

/// The hook that stops an ownership command once the remote has taken its push, before bead hears
/// back: the lost acknowledgement.
const CRASH_AFTER_PUSH: &str = "BEAD_TEST_CRASH_AFTER_PUSH";

/// Compact, ASCII-escaped JSON.
const COMPACT_ASCII: Format = Format {
    style: Style::Compact,
    ascii: true,
    sorted: false,
};

/// The id of the issue a `--json` result is: `result["id"]`.
fn id(result: Option<Json>) -> String {
    let result = result.expect("bead printed a result");
    result["id"].as_str().expect("an id").to_owned()
}

/// `remote_blob`: the bytes `git show REVISION:PATH` prints from the bare repository `remote`.
fn remote_blob(remote: &Path, revision_path: &str) -> Vec<u8> {
    let output = Command::new("git")
        .arg("--git-dir")
        .arg(remote)
        .args(["show", revision_path])
        .stdin(Stdio::null())
        .output()
        .expect("git starts");
    assert!(
        output.status.success(),
        "git show {revision_path} failed ({}): {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

/// The entries of `dir` whose names end with `suffix`, in directory order:
/// none when `dir` is not a directory.
fn glob(dir: &Path, suffix: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .map(|entry| entry.expect("a directory entry").path())
        .filter(|path| {
            let name = path.file_name().expect("a name");
            name.as_encoded_bytes().ends_with(suffix.as_bytes())
        })
        .collect()
}

/// The entries of `machine`'s `events` folder, one folder per machine, in directory order.
fn replicas(machine: &Machine) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(machine.store().join("events")) else {
        return Vec::new();
    };
    entries
        .map(|entry| entry.expect("a directory entry").path())
        .collect()
}

/// The attempt files in `machine`'s store, in directory order, under `events/*/claims/`.
fn attempts(machine: &Machine) -> Vec<PathBuf> {
    let replicas = replicas(machine).into_iter();
    replicas
        .flat_map(|replica| glob(&replica.join("claims"), ".json"))
        .collect()
}

/// The journal segments in `machine`'s store, in directory order, under `events/*/`.
fn segments(machine: &Machine) -> Vec<PathBuf> {
    let replicas = replicas(machine).into_iter();
    replicas
        .flat_map(|replica| glob(&replica, ".jsonl"))
        .collect()
}

/// The file's text, which must be UTF-8.
fn read_text(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
}

/// Splits text at newline, CRLF, carriage return, vertical tab, form feed, U+001C–U+001E, U+0085,
/// U+2028 and U+2029, without a final empty line.
fn splitlines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        let breaks = matches!(
            c,
            '\n' | '\r'
                | '\x0b'
                | '\x0c'
                | '\x1c'
                | '\x1d'
                | '\x1e'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        );
        if !breaks {
            continue;
        }
        lines.push(&text[start..at]);
        start = at + c.len_utf8();
        if c == '\r' && chars.next_if(|&(_, next)| next == '\n').is_some() {
            start += 1;
        }
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

/// Q01: a damaged foreign attempt must be refused or repaired, never republished.
#[test]
fn q01_damaged_immutable_attempt_cannot_replace_remote_transaction() {
    let sandbox = crate::sandbox();
    let [a, b, c] = ["q01-a", "q01-b", "q01-c"].map(|name| Machine::new(&sandbox, name));
    a.bead(["init", "--prefix", "ex"]);
    let key = id(a.bead(["create", "before"]));
    a.bead(["sync"]);
    b.bead(["sync"]);
    a.bead([
        "update",
        &key,
        "--claim",
        "--title",
        "accepted",
        "--add-label",
        "kept",
    ]);
    b.bead(["sync"]);

    let attempt = attempts(&b)
        .into_iter()
        .find(|path| splitlines(&read_text(path)).len() == 3)
        .expect("an attempt of three lines");
    let store = b.store();
    let rel = attempt
        .strip_prefix(&store)
        .expect("an attempt in b's store");
    let rel = rel.to_str().expect("a UTF-8 path");
    let accepted = remote_blob(sandbox.origin(), &format!("refs/beads/journal:{rel}"));
    let text = read_text(&attempt);
    std::fs::write(&attempt, format!("{}\n", splitlines(&text)[0])).unwrap();

    // A repair may succeed or a fork check may refuse.  In either case the
    // authoritative remote transaction must remain complete for a fresh peer.
    b.run(["sync", "--json"]);
    c.bead(["sync"]);
    let shown = c.show(&key);
    assert_eq!(shown["title"], Json::from("accepted"), "{shown:?}");
    assert_eq!(shown.get("labels"), Some(&json!(["kept"])), "{shown:?}");
    assert_eq!(
        remote_blob(sandbox.origin(), &format!("refs/beads/journal:{rel}")),
        accepted
    );
}

/// Q01: one malformed line must make the whole ownership attempt inert.
#[test]
fn q01_malformed_attempt_is_inert_as_a_whole() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "q01-malformed");
    a.bead(["init", "--prefix", "ex"]);
    let key = id(a.bead(["create", "survivor"]));
    let records: Vec<Json> = segments(&a)
        .iter()
        .flat_map(|path| {
            let text = read_text(path);
            let lines = splitlines(&text).into_iter();
            lines
                .map(|line| parse_json(line).expect("a journal line is JSON"))
                .collect::<Vec<_>>()
        })
        .collect();
    let created = records
        .iter()
        .find(|event| event["op"] == Json::from("create"))
        .expect("the create event");
    let created = created["id"].clone();
    let claim_id = "019a4444-0001-7000-8000-000000000001";
    let claim = json!({
        "v": 1,
        "id": claim_id,
        "replica": "corrupt",
        "deps": [(created.clone())],
        "actor": {"agent": "test", "thread": "corrupt-thread"},
        "op": "claim",
        "issue": created,
        "data": {"mode": "online", "generation": 1},
    });
    let path = a
        .store()
        .join("events")
        .join("corrupt")
        .join("claims")
        .join(format!("{claim_id}.json"));
    // Create parents as needed; the folder itself must be new.
    let folder = path.parent().expect("the attempt's folder");
    std::fs::create_dir_all(folder.parent().expect("the replica's folder")).unwrap();
    std::fs::create_dir(folder).unwrap();
    let mut bytes = write(&claim, COMPACT_ASCII).unwrap().into_bytes();
    bytes.extend_from_slice(b"\n{");
    std::fs::write(&path, bytes).unwrap();

    let shown = a.show(&key);
    assert!(
        shown.get("claim").is_none(),
        "a valid prefix of a malformed attempt became confirmed"
    );
    assert!(a.bead(["info"]).unwrap()["integrity"].truthy());
}

/// Q02: retrying an accepted claiming parent move must not remove the new parent.
#[test]
fn q02_retry_after_accepted_claiming_parent_move_keeps_parent() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "q02-a");
    a.bead(["init", "--prefix", "ex"]);
    let first = id(a.bead(["create", "first parent"]));
    let second = id(a.bead(["create", "second parent"]));
    let child = id(a.bead(["create", "child", "--parent", &first]));
    a.bead(["sync"]);
    let peer = Machine::new(&sandbox, "q02-peer");
    peer.bead(["sync"]);

    let move_child = ["update", &child, "--claim", "--parent", &second, "--json"];
    let crashed = a.cmd(move_child).env(CRASH_AFTER_PUSH, "1").run();
    assert_eq!(crashed.returncode(), 99, "{}", crashed.stderr);
    assert_eq!(a.show(&child)["parent"], Json::from(first.as_str()));
    let retry = a.run(move_child);
    assert_eq!(retry.returncode(), 0, "{}", retry.stderr);
    let second = Json::from(second);
    assert_eq!(a.show(&child).get("parent"), Some(&second));
    peer.bead(["sync"]);
    assert_eq!(peer.show(&child).get("parent"), Some(&second));
}

/// Q02: recovery must not append a second close with new time/provenance.
#[test]
fn q02_retry_after_accepted_claiming_close_emits_one_close() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "q02-close");
    a.bead(["init", "--prefix", "ex"]);
    let key = id(a.bead(["create", "close once"]));
    a.bead(["sync"]);
    let close = ["update", &key, "--claim", "--status", "closed", "--json"];
    let crashed = a.cmd(close).env(CRASH_AFTER_PUSH, "1").run();
    assert_eq!(crashed.returncode(), 99, "{}", crashed.stderr);
    let retry = a.run(close);
    assert_eq!(retry.returncode(), 0, "{}", retry.stderr);
    let log = a.bead(["log", &key, "--limit", "0"]).unwrap();
    let events = log.as_array().expect("bead log prints a list");
    let closes = events
        .iter()
        .filter(|event| event["op"] == Json::from("close"))
        .count();
    assert_eq!(closes, 1, "the recovered command appended a second close");
}

/// Q04: an older client must not replace a future manifest/tree at the remote tip.
#[test]
fn q04_future_manifest_is_refused_without_remote_rewrite() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "q04-a");
    a.bead(["init", "--prefix", "ex"]);
    a.bead(["create", "published"]);
    a.bead(["sync"]);

    let attacker = sandbox.clone("q04-attacker", None);
    let at = Some(attacker.as_path());
    git(
        [
            "fetch",
            "-q",
            "origin",
            "refs/beads/journal:refs/remotes/origin/beads-journal",
        ],
        at,
    );
    git(
        [
            "checkout",
            "-q",
            "--detach",
            "refs/remotes/origin/beads-journal",
        ],
        at,
    );
    let store = a.store();
    let workspace = store.file_name().expect("the store's name").to_str();
    let manifest = json!({"format": 2, "workspace": (workspace.expect("a UTF-8 name"))});
    let manifest = write(&manifest, Format::ASCII).unwrap() + "\n";
    std::fs::write(attacker.join("manifest.json"), manifest).unwrap();
    std::fs::write(
        attacker.join("future-state.bin"),
        "future state must survive\n",
    )
    .unwrap();
    git(["add", "-A"], at);
    git(
        [
            "-c",
            "user.name=review",
            "-c",
            "user.email=review@example.invalid",
            "commit",
            "-q",
            "-m",
            "future tracker format",
        ],
        at,
    );
    git(
        ["push", "-q", "--force", "origin", "HEAD:refs/beads/journal"],
        at,
    );
    let origin = sandbox.origin().to_str().expect("a UTF-8 path");
    // Runs git against the sandbox's bare remote.
    let at_origin = |args: &[&str]| git(["--git-dir", origin].iter().chain(args), None);
    let future_tip = at_origin(&["rev-parse", "refs/beads/journal"]);
    let future_tip = future_tip.trim();

    let fresh = Machine::new(&sandbox, "q04-fresh");
    let result = fresh.run(["sync", "--json"]);
    let after = at_origin(&["rev-parse", "refs/beads/journal"]);
    let after = after.trim();
    let listing = at_origin(&["ls-tree", "-r", "--name-only", after]);
    let names = splitlines(&listing);
    let shown = at_origin(&["show", &format!("{after}:manifest.json")]);
    let manifest = parse_json(&shown).expect("the manifest is JSON");
    assert_ne!(
        result.returncode(),
        0,
        "future format was accepted: {}",
        result.stdout
    );
    assert_eq!(after, future_tip, "sync rewrote the future-format head");
    assert_eq!(manifest["format"], Json::from(2));
    assert!(names.contains(&"future-state.bin"), "{names:?}");
}
