//! The unpublished count every command gives (roll-lqjj.47): what `unpublished_count` says is what
//! the journal and `meta.json` say, counted the slow way, on every path a store takes to a count,
//! and whatever the memo it is helped by holds.
//!
//! The count reads the memo's entries of the segments alone, by a read that checks the rest of the
//! memo for its shape and makes nothing of it, and names the attempt files by their bytes. None of
//! that may change a number: the memo may be missing, older than the files, damaged anywhere, or
//! behind a rotation, a write, a failed sync or a claim, and the count is the same. The count
//! takes the lines of a segment the memo vouches for from the memo, and not from the file.
//! `count_foreign` has the same oracle on two machines.

use std::fs::{FileTimes, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use beadroll_core::json::{self, Value};
use beadroll_store::{Workspace, memo_path};
use beadroll_sync::unpublished_count;
use beadroll_test_support::{Machine, Sandbox};

use crate::support::{sandbox, workspace};

/// The journal files of the store at `dir` by their paths in it: the segments with their lines,
/// and the attempt files, found by walking `events/` and reading every file.
fn journal(dir: &Path) -> (Vec<(String, usize)>, Vec<String>) {
    let (mut segments, mut attempts) = (Vec::new(), Vec::new());
    for machine in std::fs::read_dir(dir.join("events")).expect("events") {
        let machine = machine.expect("a journal").path();
        let name = machine
            .file_name()
            .and_then(|n| n.to_str())
            .expect("a name");
        for file in std::fs::read_dir(&machine).expect("a journal") {
            let file = file.expect("a file").path();
            let leaf = file.file_name().and_then(|n| n.to_str()).expect("a name");
            if leaf.ends_with(".jsonl") {
                let lines = std::fs::read(&file).expect("a segment");
                let lines = lines.iter().filter(|b| **b == b'\n').count();
                segments.push((format!("events/{name}/{leaf}"), lines));
            } else if leaf == "claims" {
                for attempt in std::fs::read_dir(&file).expect("claims") {
                    let attempt = attempt.expect("an attempt").path();
                    let leaf = attempt
                        .file_name()
                        .and_then(|n| n.to_str())
                        .expect("a name");
                    attempts.push(format!("events/{name}/claims/{leaf}"));
                }
            }
        }
    }
    (segments, attempts)
}

/// The count as the journal and `meta.json` give it, counted the slow way: each segment's lines
/// less the lines published, and each attempt file not published. `None` without a remote, and
/// when `meta.json` holds published counts of no segment although the store has synced.
fn counted_slowly(ws: &Workspace) -> Option<String> {
    ws.remote(None)
        .ok()
        .flatten()
        .filter(|url| !url.is_empty())?;
    let text = std::fs::read(ws.dir.join("meta.json")).unwrap_or_default();
    let meta = match json::parse(&text) {
        Ok(parsed) => parsed.value,
        Err(_) => Value::Object(Box::default()),
    };
    let published = match meta.get("published") {
        Some(Value::Object(published)) => Some(published),
        _ if meta.get("last_sync").is_some_and(Value::truthy) => return None,
        _ => None,
    };
    let known = |name: &str| -> i64 {
        match published.and_then(|published| published.get(name)) {
            Some(Value::Int(n)) => n.to_string().parse().expect("a count"),
            Some(other) => panic!("a count that is not a number: {other:?}"),
            None => 0,
        }
    };
    let (segments, attempts) = journal(&ws.dir);
    let behind: i64 = (segments.iter())
        .map(|(name, lines)| (*lines as i64 - known(name)).max(0))
        .sum();
    let unpublished = attempts
        .iter()
        .filter(|name| published.is_none_or(|published| !published.contains_key(*name)))
        .count();
    Some((behind + unpublished as i64).to_string())
}

/// What `a`'s store says of the count, and that it is what counting the slow way says. The store
/// is opened again each time: a listing a process keeps is not what another process just wrote.
pub(crate) fn counts(a: &Machine, sandbox: &Sandbox, when: &str) -> Option<String> {
    let ws = workspace(a, sandbox.origin(), &[]);
    let counted = unpublished_count(&ws)
        .expect("a count")
        .map(|n| n.to_string());
    assert_eq!(counted, counted_slowly(&ws), "{when}");
    counted
}

/// A change to the memo of `ws`'s store, which cannot make the count anything else.
pub(crate) fn damage(ws: &Workspace, how: &str) {
    let memo = memo_path(ws);
    match how {
        "missing" => std::fs::remove_file(&memo).expect("a memo"),
        "older than every file" => {
            let file = OpenOptions::new().write(true).open(&memo).expect("a memo");
            let earlier = SystemTime::now() - Duration::from_secs(3600);
            file.set_times(FileTimes::new().set_modified(earlier))
                .expect("a time");
        }
        "trusted for every file" => {
            // written after every file it remembers, it vouches for each of them
            let file = OpenOptions::new().write(true).open(&memo).expect("a memo");
            let later = SystemTime::now() + Duration::from_secs(3600);
            file.set_times(FileTimes::new().set_modified(later))
                .expect("a time");
        }
        "garbage" => std::fs::write(&memo, b"bead journal memo 1\nnot a record").expect("a memo"),
        "an attempt file's fields" => {
            // a line as a memo is not written, among lines of files the count reads
            let text = std::fs::read_to_string(&memo).expect("a memo");
            let at = text.find("/claims/").expect("an attempt file in the memo");
            let (head, tail) = text.split_at(at);
            let field = tail.find('\0').expect("a record") + 1;
            let (path, fields) = tail.split_at(field);
            let damaged = format!("{head}{path}x{fields}");
            std::fs::write(&memo, damaged).expect("a memo");
        }
        other => panic!("{other}"),
    }
}

/// The count at every stage of a store that publishes, writes offline, rotates, claims, loses its
/// remote and syncs again: always what the slow count says, and the memo, missing or damaged in
/// every way the count can see, never moves it.
#[test]
fn the_count_is_the_slow_count_on_every_path_and_whatever_the_memo_holds() {
    let sandbox = sandbox();
    let origin = sandbox.origin().to_path_buf();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let key = a.bead(["create", "x"]).expect("a create")["id"]
        .as_str()
        .expect("a key")
        .to_owned();
    assert_eq!(counts(&a, &sandbox, "published"), Some("0".into()));

    a.bead(["comments", "add", &key, "one", "--offline"]);
    assert_eq!(counts(&a, &sandbox, "a local write"), Some("1".into()));
    a.bead(["comments", "add", &key, "two", "--offline"]);
    assert_eq!(counts(&a, &sandbox, "two local writes"), Some("2".into()));

    // a rotated journal: full segments the memo has never been told of
    for text in ["three", "four"] {
        a.cmd(["comments", "add", &key, text, "--offline"])
            .env("BEAD_ROTATE_BYTES", "1")
            .run();
    }
    let rotated = counts(&a, &sandbox, "a rotated journal").expect("a count");
    assert_eq!(rotated, "4");
    let claim = a.run(["claim", &key, "--offline"]);
    assert_eq!(claim.returncode(), 0, "{}", claim.stderr);
    let claimed = counts(&a, &sandbox, "a claim offline").expect("a count");
    assert!(
        claimed.parse::<i64>().unwrap() > 4,
        "its attempt file counts"
    );

    // the memo of a store that has not synced since is whatever the last sync left; then it is
    // taken away or damaged, and the number does not move
    for how in ["missing", "older than every file", "garbage"] {
        let ws = workspace(&a, &origin, &[]);
        if memo_path(&ws).exists() {
            damage(&ws, how);
        }
        assert_eq!(counts(&a, &sandbox, how), Some(claimed.clone()), "{how}");
    }

    // a sync that publishes, and the memo it writes: segments and attempt files
    a.bead(["sync"]);
    assert_eq!(counts(&a, &sandbox, "a sync"), Some("0".into()));
    for how in [
        "trusted for every file",
        "an attempt file's fields",
        "older than every file",
        "missing",
    ] {
        damage(&workspace(&a, &origin, &[]), how);
        assert_eq!(counts(&a, &sandbox, how), Some("0".into()), "{how}");
    }
    a.bead(["sync"]);
    damage(&workspace(&a, &origin, &[]), "trusted for every file");
    a.bead(["comments", "add", &key, "five", "--offline"]);
    assert_eq!(counts(&a, &sandbox, "a write after it"), Some("1".into()));

    // a sync that fails leaves the count where it was; the next one publishes
    let away = away_from(&origin);
    std::fs::rename(&origin, &away).expect("the remote goes");
    let failed = a.run(["sync"]);
    assert_ne!(failed.returncode(), 0, "the remote is gone");
    a.bead(["comments", "add", &key, "six", "--offline"]);
    assert_eq!(counts(&a, &sandbox, "a failed sync"), Some("2".into()));
    std::fs::rename(&away, &origin).expect("the remote is back");
    a.bead(["sync"]);
    assert_eq!(counts(&a, &sandbox, "synced again"), Some("0".into()));

    // meta.json as a store that never published has it, and as one that lost its counts
    let meta = a.store().join("meta.json");
    let original = std::fs::read_to_string(&meta).expect("meta.json");
    let Value::Object(mut kept) = json::parse(original.as_bytes()).expect("json").value else {
        panic!("an object");
    };
    kept.shift_remove("published");
    kept.shift_remove("last_sync");
    let never = json::write(&Value::Object(kept.clone()), json::Format::INDENTED).expect("json");
    std::fs::write(&meta, never).expect("meta.json");
    let all = counts(&a, &sandbox, "never published").expect("a count");
    let (segments, attempts) = journal(&workspace(&a, &origin, &[]).dir);
    let lines: usize = segments.iter().map(|(_, lines)| lines).sum();
    assert_eq!(all, (lines + attempts.len()).to_string());
    kept.insert("last_sync".into(), Value::from("2026-10-16T00:00:00Z"));
    let lost = json::write(&Value::Object(kept), json::Format::INDENTED).expect("json");
    std::fs::write(&meta, lost).expect("meta.json");
    assert_eq!(counts(&a, &sandbox, "synced, counts lost"), None);
}

/// The count takes the lines of a segment the memo vouches for from the memo: a memo written after
/// the segment, whose record of it is the segment's own, is believed when it says the segment has
/// five lines more than it has. A count that read the segments again would be the 22 ms at
/// 200,000 events that roll-lqjj.47 was about, and would give nothing the other tests can tell
/// from this one. (A memo that is made unreadable to the count by chmod would not do: that changes
/// the file's ctime, and the memo no longer vouches for it.)
#[test]
fn a_vouched_segment_is_not_read_by_the_count() {
    let sandbox = sandbox();
    let origin = sandbox.origin().to_path_buf();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let key = a.bead(["create", "x"]).expect("a create")["id"]
        .as_str()
        .expect("a key")
        .to_owned();
    a.bead(["comments", "add", &key, "one", "--offline"]);
    a.bead(["sync"]);
    assert_eq!(counts(&a, &sandbox, "synced"), Some("0".into()));

    // the record of the first segment: path, then len, mtime, ctime, inode, dev, lines and blob
    let ws = workspace(&a, &origin, &[]);
    let memo = memo_path(&ws);
    let text = std::fs::read_to_string(&memo).expect("a memo");
    let (head, tail) = text.split_once(".jsonl\0").expect("a segment in the memo");
    let (fields, rest) = tail.split_once('\n').expect("a record");
    let mut fields: Vec<String> = fields.split(' ').map(str::to_owned).collect();
    let lines: usize = fields[5].parse().expect("the lines remembered");
    fields[5] = (lines + 5).to_string();
    let believed = format!("{head}.jsonl\0{}\n{rest}", fields.join(" "));
    std::fs::write(&memo, believed).expect("a memo");
    damage(&ws, "trusted for every file");

    let ws = workspace(&a, &origin, &[]);
    let counted = unpublished_count(&ws)
        .expect("a count")
        .map(|n| n.to_string());
    assert_eq!(counted, Some("5".into()), "the memo is believed");
}

/// Where the remote goes while it is unreachable.
pub(crate) fn away_from(origin: &Path) -> PathBuf {
    let mut name = origin.file_name().expect("a name").to_os_string();
    name.push(".away");
    origin.with_file_name(name)
}
