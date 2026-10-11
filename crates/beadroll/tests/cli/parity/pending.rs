//! What a store keeps between commands, read as Python's bead read it: pending attempts and their
//! receipts, which a sync settles (`docs/RUST-PORT.md` §2.3j), a pending file that is not an
//! attempt, and the published counts in `meta.json`.

use std::os::unix::fs::PermissionsExt as _;

use beadroll_core::json::{Format, Object, Value, write};
use beadroll_core::replay::replay;
use beadroll_store::{Workspace, find_workspace, load_journal, write_atomic, write_attempt_file};
use beadroll_sync::{Link, PushOutcome, Tx, locked};
use beadroll_test_support::{Machine, code, parse_json};

use super::{id, json, settings};

/// What settling left of a store's pending attempts: the files in `pending/` by suffix, and whether
/// the issue is claimed.
fn settled(machine: &Machine, key: &str) -> (usize, usize, bool) {
    let pending = machine.store().join("pending");
    let names: Vec<String> = std::fs::read_dir(&pending)
        .map(|entries| {
            entries
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    let count = |suffix: &str| names.iter().filter(|n| n.ends_with(suffix)).count();
    let shown = json(machine, &["show", key]);
    (
        count(".json"),
        count(".command"),
        shown[0].get("claim").is_some(),
    )
}

/// A pending attempt and its receipt that a claim left when it stopped (accepted with its
/// acknowledgement lost, or never pushed) are settled by the next sync: confirmed, or fenced and
/// dropped. The pending file goes; an accepted attempt's receipt stays. Until the cutover Python's
/// bead left them and either implementation settled them.
#[test]
fn pending_attempts_a_stopped_claim_left_are_settled_by_sync() {
    let cases = [
        ("BEAD_TEST_CRASH_AFTER_PUSH", 99, (0, 1, true)),
        ("BEAD_TEST_CRASH_BEFORE_PUSH", 98, (0, 0, false)),
    ];
    for (hook, status, expected) in cases {
        let sandbox = crate::sandbox();
        let m = Machine::new(&sandbox, "m");
        json(&m, &["init", "--prefix", "ex"]);
        let x = id(&json(&m, &["create", "x"]));
        json(&m, &["sync"]);
        let stopped = m.cmd(["claim", &x]).env(hook, "1").run();
        assert_eq!(stopped.returncode(), status, "{hook}: {}", stopped.stderr);
        assert_eq!(settled(&m, &x), (1, 1, false), "{hook}: before settling");
        json(&m, &["sync"]);
        assert_eq!(settled(&m, &x), expected, "{hook}, settled");
    }
}

/// A claim of `issue` by `ws`'s actor written as an ownership command writes it, in-process: the
/// receipt first, then the pending attempt; with `publish`, pushed with the journal, its answer
/// lost (the attempt stays pending).
fn leave_pending(ws: &Workspace, actor: &Value, issue: &str, publish: bool) {
    let remote = ws.endpoint(None).unwrap().unwrap();
    let mut link = Link::new(ws, &remote).unwrap();
    locked(ws, || -> Result<(), beadroll_sync::Error> {
        let tip = link.fetch()?;
        link.merge(tip.as_deref())?;
        let mut tx = Tx::new(ws, &beadroll_store::Actor::writable(actor.clone()))?;
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
            &ws.settings.failpoints,
        )?;
        write_attempt_file(
            &folder.join(format!("{first}.json")),
            &tx.ownership,
            &ws.settings.failpoints,
        )?;
        if publish {
            let outcome = link.publish(tip.as_deref(), "claim", &tx.ownership, false)?;
            assert_eq!(outcome, PushOutcome::Ok);
        }
        Ok(())
    })
    .unwrap();
}

/// A pending attempt and receipt written in this process as the ownership protocol writes them
/// (receipt, then attempt, then the push whose answer is lost, or no push) are settled by the
/// binary's sync. Until the cutover Python's sync settled them the same way.
#[test]
fn pending_attempts_left_in_process_are_settled_by_sync() {
    for (publish, expected) in [(true, (0, 1, true)), (false, (0, 0, false))] {
        let sandbox = crate::sandbox();
        let r = crate::rust_only(&sandbox, "r");
        json(&r, &["init", "--prefix", "ex"]);
        let x = id(&json(&r, &["create", "x"]));
        json(&r, &["sync"]);
        let settings = settings(&r);
        let ws = find_workspace(&settings).unwrap();
        let st = replay(load_journal(&ws).unwrap()).unwrap();
        let uid = st.resolve(&x).unwrap().uid().to_string();
        let actor = beadroll_test_support::json!({"agent": "codex", "thread": "r-thread-0001"});
        leave_pending(&ws, &actor, &uid, publish);
        assert_eq!(settled(&r, &x), (1, 1, false), "before settling");
        json(&r, &["sync"]);
        assert_eq!(settled(&r, &x), expected, "published={publish}, settled");
    }
}

/// A pending file that is not an attempt is swept by the store lock's holder; one that cannot be
/// removed is an IO error naming it, never a silent success, and stays until it can be removed.
#[test]
fn an_unremovable_invalid_pending_file_is_an_io_error() {
    let sandbox = crate::sandbox();
    let r = Machine::new(&sandbox, "r");
    json(&r, &["init", "--prefix", "ex"]);
    json(&r, &["sync"]);
    let pending = r.store().join("pending");
    std::fs::create_dir_all(&pending).unwrap();
    let junk = pending.join("0190aaaa-0000-7000-8000-000000000001.json");
    std::fs::write(&junk, "not an attempt\n").unwrap();
    let mode = |bits: u32| {
        std::fs::set_permissions(&pending, std::fs::Permissions::from_mode(bits)).unwrap();
    };
    mode(0o500);
    let synced = r.run(["sync", "--json"]);
    mode(0o700);
    assert_eq!(synced.returncode(), 1, "{}", synced.stdout);
    assert_eq!(code(&synced), "IO", "{}", synced.stderr);
    let error = parse_json(&synced.stderr).unwrap();
    let message = error["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("0190aaaa-0000-7000-8000-000000000001.json"),
        "{message}"
    );
    assert!(junk.exists(), "the file stays until it can be removed");
}

/// What Python's bead answered `list` with, captured before the cutover, when `meta.json` held
/// each of these as the published count of the store's one segment, which holds three events: the
/// unpublished count, absent when nothing is. Python reads a count as `int(value or 0)` and
/// subtracts with its exact arithmetic, so a count far below zero leaves a huge unpublished count,
/// and a float or a string is read as `int()` reads it.
const COUNTS: [(&str, Option<&str>); 10] = [
    ("-9223372036854775808", Some("9223372036854775811")),
    (
        "-99999999999999999999999999999999999999999999",
        Some("100000000000000000000000000000000000000000002"),
    ),
    ("1e20", None),
    ("-2.5", Some("5")),
    ("\" 1_0 \"", None),
    ("true", Some("2")),
    ("null", Some("3")),
    ("1", Some("2")),
    ("0", Some("3")),
    ("[]", Some("3")),
];

/// A store of `machine` with one segment of three events, published: its `meta.json`, what that
/// file held, the segment's path in the store, and the text that records its count.
fn published_three(machine: &Machine) -> (std::path::PathBuf, String, String, String) {
    json(machine, &["init", "--prefix", "ex"]);
    json(machine, &["create", "one"]);
    json(machine, &["create", "two"]);
    let meta = machine.store().join("meta.json");
    let original = std::fs::read_to_string(&meta).unwrap();
    let journals = std::fs::read_dir(machine.store().join("events")).unwrap();
    let replica = journals.map(|entry| entry.unwrap().file_name()).next();
    let replica = replica.unwrap().to_str().unwrap().to_owned();
    let segment = format!("events/{replica}/000001.jsonl");
    let published = format!("\"{segment}\": 3");
    assert!(original.contains(&published), "{original}");
    (meta, original, segment, published)
}

/// What `meta.json` records as published may be anything a hand left there: bead reads each count
/// as Python's bead did and reports the unpublished count Python reported, in both output modes.
/// The mixed fleet compared the two implementations here, but with the odd count written before
/// the real one, whose last value won: the counts were never read.
#[test]
fn unpublished_counts_read_meta_json_as_python_did() {
    let sandbox = crate::sandbox();
    let r = Machine::new(&sandbox, "r");
    let (meta, original, segment, published) = published_three(&r);
    for (count, unpublished) in COUNTS {
        let edited = original.replacen(&published, &format!("\"{segment}\": {count}"), 1);
        std::fs::write(&meta, edited).unwrap();
        let listed = json(&r, &["list"]);
        let said = listed.get("unpublished");
        let said = said.map(|n| write(n, Format::COMPACT).unwrap());
        assert_eq!(said.as_deref(), unpublished, "published count {count}");
        let human = r.run(["list"]);
        let tail = " events not yet published: bead sync";
        match unpublished {
            Some(n) => {
                let last = human.stdout.lines().last();
                assert_eq!(last, Some(format!("{n}{tail}").as_str()), "{count}");
            }
            None => assert!(!human.stdout.contains(tail), "{count}: {}", human.stdout),
        }
    }
}

/// A published count that is not a number is an IO error naming it, where Python's bead died with
/// a traceback (`docs/RUST-PORT.md` §5 row 26, `REVIEWING.md` §2): the Rust bead's own answer, so
/// the machine runs the binary this package builds whatever `BEAD_UNDER_TEST` says.
#[test]
fn a_published_count_that_is_not_a_number_is_an_io_error() {
    let sandbox = crate::sandbox();
    let r = crate::rust_only(&sandbox, "r");
    let (meta, original, segment, published) = published_three(&r);
    let edited = original.replacen(&published, &format!("\"{segment}\": \"x\""), 1);
    std::fs::write(&meta, edited).unwrap();
    let refused = r.run(["list", "--json"]);
    assert_eq!(code(&refused), "IO", "{}", refused.stderr);
    let wanted = format!("the published count of {segment} is not a number");
    assert!(refused.stderr.contains(&wanted), "{}", refused.stderr);
    let quiet = "an error prints nothing on standard output";
    assert_eq!(refused.stdout, "", "{quiet}");
}
