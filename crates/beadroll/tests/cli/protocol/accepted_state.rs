//! What the remote accepted stays as it accepted it. An attempt damaged in one store never replaces
//! the one the remote accepted, and an attempt with a malformed line is inert as a whole (Q01); the
//! retry of an accepted claiming update repeats none of its effects (Q02, on the crash hook
//! `BEAD_TEST_CRASH_AFTER_PUSH`, which any bead under test implements: exit status 99 once the
//! remote has taken the push); a manifest of a future format at the remote is refused, never
//! rewritten (Q04). Q03 interrupts a release between its confirmation and its cleanup, which takes
//! an in-crate API test: beadroll-sync's `protocol::retried_release`.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use beadroll_core::json::{Format, Style, write};
use beadroll_store::glob_ending;
use beadroll_test_support::{
    Json, Machine, failure_code, git, json, json_line, parse_json, printed_id, segments,
};

/// The hook that stops an ownership command once the remote has taken its push, before bead hears
/// back: the lost acknowledgement.
const CRASH_AFTER_PUSH: &str = "BEAD_TEST_CRASH_AFTER_PUSH";

/// Compact, ASCII-escaped JSON.
const COMPACT_ASCII: Format = Format {
    style: Style::Compact,
    ascii: true,
    sorted: false,
};

/// `remote_blob`: the bytes `git show REVISION:PATH` prints from the bare repository `remote`.
fn remote_blob(remote: &Path, revision_path: &str) -> Vec<u8> {
    let output = beadroll_test_support::git_command()
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
        .flat_map(|replica| glob_ending(&replica.join("claims"), ".json"))
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
fn damaged_immutable_attempt_cannot_replace_remote_transaction() {
    let sandbox = crate::sandbox();
    let [a, b, c] = ["q01-a", "q01-b", "q01-c"].map(|name| Machine::new(&sandbox, name));
    a.bead(["init", "--prefix", "ex"]);
    let key = printed_id(a.bead(["create", "before"]));
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
fn malformed_attempt_is_inert_as_a_whole() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "q01-malformed");
    a.bead(["init", "--prefix", "ex"]);
    let key = printed_id(a.bead(["create", "survivor"]));
    let records: Vec<Json> = segments(&a.store())
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
fn retry_after_accepted_claiming_parent_move_keeps_parent() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "q02-a");
    a.bead(["init", "--prefix", "ex"]);
    let first = printed_id(a.bead(["create", "first parent"]));
    let second = printed_id(a.bead(["create", "second parent"]));
    let child = printed_id(a.bead(["create", "child", "--parent", &first]));
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
fn retry_after_accepted_claiming_close_emits_one_close() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "q02-close");
    a.bead(["init", "--prefix", "ex"]);
    let key = printed_id(a.bead(["create", "close once"]));
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
fn future_manifest_is_refused_without_remote_rewrite() {
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
    let manifest = json_line(&manifest) + "\n";
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
    let future_tip = sandbox.remote_journal().expect("the future journal");

    let fresh = Machine::new(&sandbox, "q04-fresh");
    let result = fresh.run(["sync", "--json"]);
    let after = sandbox.remote_journal().expect("a journal");
    let listing = sandbox.origin_git(["ls-tree", "-r", "--name-only", after.as_str()]);
    let names = splitlines(&listing);
    let shown = sandbox.origin_git(["show", format!("{after}:manifest.json").as_str()]);
    let manifest = parse_json(&shown).expect("the manifest is JSON");
    assert_eq!(
        (result.returncode(), failure_code(&result).as_deref()),
        (1, Some("FORMAT_UNSUPPORTED")),
        "future format was not refused as such: {}{}",
        result.stdout,
        result.stderr
    );
    assert_eq!(after, future_tip, "sync rewrote the future-format head");
    assert_eq!(manifest["format"], Json::from(2));
    assert!(names.contains(&"future-state.bin"), "{names:?}");
}
