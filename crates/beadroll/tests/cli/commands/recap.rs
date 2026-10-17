//! Recap bookmarks delivered events across repositories, independent of their timestamps.

mod delivery;

use std::collections::BTreeSet;

use beadroll_test_support::{Json, Machine, code, git, json, parse_json};

use super::bp_test::segments;

fn create(machine: &Machine, title: &str) -> String {
    machine.bead(["create", title]).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn events(answer: &Json) -> Vec<&Json> {
    answer["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|ws| ws["events"].as_array().unwrap())
        .collect()
}

fn recap(machine: &Machine) -> Json {
    machine.bead(["recap"]).unwrap()
}

#[test]
fn first_read_preview_repeat_and_all_preserve_event_provenance() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    git(["checkout", "-b", "feature/recap"], Some(a.checkout()));
    let key = create(&a, "Original");
    a.bead(["update", &key, "--title", "Renamed", "-p", "1"]);
    a.cmd(["comments", "add", &key, "The reason", "--actor", "Alex"])
        .agent("claude")
        .thread("whole-thread-id-123456789")
        .json();
    a.bead(["claim", &key]);
    a.bead(["close", &key, "-r", "Shipped"]);
    let info = a.bead(["info"]).unwrap();
    let machine = info["machine"].as_str().unwrap();
    a.bead(["machines", "--name", &format!("{machine}=laptop")]);
    let preview = a.bead(["recap", "--no-advance"]).unwrap();
    assert!(!a.home().join(".recap.json").exists());
    assert!(!a.home().join(".recap.lock").exists());
    check_provenance(&a, &key, machine, &preview);
    assert_eq!(recap(&a), preview);
    let cursor = std::fs::read(a.home().join(".recap.json")).unwrap();
    assert!(events(&recap(&a)).is_empty());
    assert_eq!(std::fs::read(a.home().join(".recap.json")).unwrap(), cursor);
    assert_eq!(a.bead(["recap", "--all", "--no-advance"]).unwrap(), preview);
    assert_eq!(a.bead(["recap", "--all"]).unwrap(), preview);
    a.bead(["reopen", &key]);
    assert_eq!(events(&recap(&a))[0]["op"].as_str(), Some("reopen"));
    a.bead(["delete", &key, "-r", "Retired"]);
    let deleted = recap(&a);
    assert_eq!(
        events(&deleted)[0]["summary"].as_str(),
        Some("delete: Retired")
    );
    assert_eq!(events(&deleted)[0]["title"].as_str(), Some("Renamed"));
    assert!(a.run(["recap"]).stdout.contains("No new activity"));
}

fn check_provenance(a: &Machine, key: &str, machine: &str, preview: &Json) {
    let commit = git(["rev-parse", "HEAD"], Some(a.checkout()));
    let rows = events(preview);
    assert_eq!(
        rows.iter()
            .map(|r| r["op"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["create", "set", "comment", "claim", "close"]
    );
    for row in &rows {
        assert_eq!(row["machine"].as_str(), Some(machine));
        assert_eq!(row["alias"].as_str(), Some("laptop"));
        assert_eq!(row["branch"].as_str(), Some("feature/recap"));
        assert_eq!(row["commit"].as_str(), Some(commit.trim()));
        assert_eq!(row["issue"].as_str(), Some(key));
        assert_eq!(row["title"].as_str(), Some("Renamed"));
    }
    assert_eq!(rows[2]["agent"].as_str(), Some("claude"));
    assert_eq!(
        rows[2]["thread"].as_str(),
        Some("whole-thread-id-123456789")
    );
    assert_eq!(rows[2]["name"].as_str(), Some("Alex"));
    assert_eq!(rows[2]["summary"].as_str(), Some("comment: The reason"));
    assert_eq!(rows[4]["summary"].as_str(), Some("close: Shipped"));
    assert!(
        rows[1]["summary"]
            .as_str()
            .unwrap()
            .contains("title=\"Renamed\"")
    );
    let shown = a.run(["recap", "--no-advance", "--color", "never"]);
    assert_eq!(shown.returncode(), 0, "{}", shown.stderr);
    for text in [
        "laptop",
        "claude",
        "whole-thread-id-123456789",
        "Alex",
        "feature/recap",
        rows[0]["commit"].as_str().unwrap(),
        "Shipped",
        "Renamed",
        "last sync:",
    ] {
        assert!(shown.stdout.contains(text), "{text}: {}", shown.stdout);
    }
}

#[test]
fn scans_all_repositories_without_a_checkout_git_or_network() {
    let sandbox = crate::sandbox();
    let other = crate::sandbox();
    let a = crate::rust_only(&sandbox, "a");
    let mut b = Machine::new(&other, "b");
    b.extra
        .insert("BEAD_HOME".into(), a.home().as_os_str().to_owned());
    a.bead(["init", "--prefix", "one"]);
    b.bead(["init", "--prefix", "two"]);
    let one = create(&a, "First repository");
    let two = create(&b, "Second repository");
    let home_before = std::fs::read_dir(a.home()).unwrap().count();
    let result = a
        .cmd(["recap", "--no-advance"])
        .cwd(sandbox.root())
        .env("PATH", "")
        .env("BEAD_WORKSPACE", "ignored")
        .env(
            "BEAD_FAILPOINTS",
            "fetch.result=exit(91);publish.before=exit(92)",
        )
        .json()
        .unwrap();
    let workspaces = result["workspaces"].as_array().unwrap();
    assert_eq!(workspaces.len(), 2);
    let ids: Vec<_> = workspaces
        .iter()
        .map(|ws| ws["workspace"].as_str().unwrap())
        .collect();
    assert!(ids[0] < ids[1]);
    assert_eq!(
        events(&result)
            .iter()
            .map(|row| row["issue"].as_str().unwrap())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([one.as_str(), two.as_str()])
    );
    assert_eq!(std::fs::read_dir(a.home()).unwrap().count(), home_before);
    assert_eq!(a.cmd(["recap"]).cwd(sandbox.root()).json().unwrap(), result);
    assert!(events(&recap(&b)).is_empty()); // the bookmark belongs to the home, not the checkout
    b.cmd(["init", "--prefix", "late", "--offline"])
        .env("BEAD_WORKSPACE", "new-store")
        .json();
    b.cmd(["create", "Newly discovered store", "--offline"])
        .env("BEAD_WORKSPACE", "new-store")
        .json();
    let discovered = recap(&a);
    assert_eq!(events(&discovered).len(), 1);
    assert_eq!(
        events(&discovered)[0]["title"].as_str(),
        Some("Newly discovered store")
    );
}

#[test]
fn bookmark_files_cannot_collide_with_existing_workspace_ids() {
    let sandbox = crate::sandbox();
    let mut a = Machine::new(&sandbox, "a");
    for workspace in ["recap.json", "recap.lock"] {
        a.extra.insert("BEAD_WORKSPACE".into(), workspace.into());
        a.bead(["init", "--prefix", "ex", "--offline"]);
        a.bead(["create", workspace, "--offline"]);
    }
    let answer = recap(&a);
    assert_eq!(answer["workspaces"].as_array().unwrap().len(), 2);
    assert_eq!(events(&answer).len(), 2);
    assert!(a.home().join("recap.json/events").is_dir());
    assert!(a.home().join("recap.lock/events").is_dir());
    assert!(a.home().join(".recap.json").is_file());
    assert!(events(&recap(&a)).is_empty());
}

#[test]
fn late_sync_from_a_known_machine_is_seen_even_with_an_older_clock() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let b = crate::rust_only(&sandbox, "b");
    a.bead(["init", "--prefix", "ex"]);
    let key = create(&a, "Shared");
    b.bead(["sync"]);
    b.bead(["comments", "add", &key, "Seen first"]);
    a.bead(["sync"]);
    let first = recap(&a);
    let previous = events(&first).last().unwrap()["at"]
        .as_str()
        .unwrap()
        .to_owned();
    b.cmd(["comments", "add", &key, "Arrived later", "--offline"])
        .env("BEAD_FAILPOINTS", "clock@always=offset(-7d)")
        .json();
    assert!(events(&recap(&a)).is_empty()); // recap itself never syncs
    b.bead(["sync"]);
    a.bead(["sync"]);
    let later = recap(&a);
    let rows = events(&later);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["summary"].as_str(), Some("comment: Arrived later"));
    assert!(rows[0]["at"].as_str().unwrap() < previous.as_str());
    assert_eq!(rows[0]["thread"].as_str(), Some("b-thread-0001"));
    assert!(events(&recap(&a)).is_empty());
    let all = a.bead(["recap", "--all"]).unwrap();
    let log = a.bead(["log", "--limit", "0"]).unwrap();
    assert_eq!(
        events(&all)
            .iter()
            .map(|row| &row["event"])
            .collect::<Vec<_>>(),
        log.as_array()
            .unwrap()
            .iter()
            .map(|row| &row["event"])
            .collect::<Vec<_>>()
    );
}

#[test]
fn ownership_attempts_are_visible_only_after_confirmation() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let key = create(&a, "Claim me");
    recap(&a);
    let stopped = a
        .cmd(["claim", &key])
        .env("BEAD_TEST_CRASH_AFTER_PUSH", "1")
        .run();
    assert_eq!(stopped.returncode(), 99, "{}", stopped.stderr);
    assert!(events(&recap(&a)).is_empty());
    a.bead(["sync"]);
    let accepted = recap(&a);
    assert_eq!(events(&accepted).len(), 1);
    assert_eq!(events(&accepted)[0]["op"].as_str(), Some("claim"));
}

#[test]
fn events_waiting_for_a_missing_cause_are_not_acknowledged() {
    let sandbox = crate::sandbox();
    let mut a = Machine::new(&sandbox, "a");
    a.extra.insert("BEAD_ROTATE_BYTES".into(), "1".into());
    a.bead(["init", "--prefix", "ex"]);
    let key = create(&a, "Waiting");
    recap(&a);
    a.bead(["comments", "add", &key, "Cause", "--offline"]);
    let cause = segments(&a.store()).pop().unwrap();
    a.bead(["comments", "add", &key, "Effect", "--offline"]);
    let held = cause.with_extension("held");
    std::fs::rename(&cause, &held).unwrap();
    let waiting = recap(&a);
    assert!(events(&waiting).is_empty());
    assert_eq!(waiting["workspaces"][0]["pending"].as_i64(), Some(1));
    std::fs::rename(held, cause).unwrap();
    let ready = recap(&a);
    assert_eq!(
        events(&ready)
            .iter()
            .map(|row| row["summary"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["comment: Cause", "comment: Effect"]
    );
}

#[test]
fn malformed_cursors_fail_without_replacing_them() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    create(&a, "Unseen");
    let path = a.home().join(".recap.json");
    for bytes in [
        "{",
        "[]",
        "{\"version\":2,\"workspaces\":{}}",
        "{\"version\":1,\"workspaces\":{\"x\":[\"not-a-uuid\"]}}",
        "{\"version\":1,\"workspaces\":[],\"future\":true}",
    ] {
        std::fs::write(&path, bytes).unwrap();
        let run = a.run(["recap", "--json"]);
        assert_eq!(run.returncode(), 1);
        assert_eq!(code(&run), "IO");
        assert!(run.stdout.is_empty());
        assert!(run.stderr.contains(".recap.json"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), bytes);
    }
    std::fs::remove_file(path).unwrap();
    assert_eq!(events(&recap(&a)).len(), 1);
}

#[test]
fn damaged_stores_report_integrity_and_keep_their_bookmark_until_repaired() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let other = crate::sandbox();
    let mut b = Machine::new(&other, "b");
    b.extra
        .insert("BEAD_HOME".into(), a.home().as_os_str().to_owned());
    a.bead(["init", "--prefix", "ex"]);
    b.bead(["init", "--prefix", "healthy"]);
    create(&a, "Seen after repair");
    let healthy = create(&b, "Read once");
    let damaged_store = a.bead(["info"]).unwrap()["path"]
        .as_str()
        .unwrap()
        .to_owned();
    let segment = segments(std::path::Path::new(&damaged_store))
        .pop()
        .unwrap();
    let original = std::fs::read(&segment).unwrap();
    let mut damaged = original.clone();
    damaged.extend_from_slice(b"not-json\n");
    std::fs::write(&segment, damaged).unwrap();
    let broken = recap(&a);
    assert_eq!(events(&broken).len(), 2);
    assert!(
        broken["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|ws| !ws["integrity"].as_array().unwrap().is_empty())
    );
    let again = recap(&a);
    assert_eq!(events(&again).len(), 1);
    assert_ne!(events(&again)[0]["issue"].as_str(), Some(healthy.as_str()));
    assert!(
        a.run(["recap"])
            .stdout
            .contains("Cursor kept for this workspace")
    );
    std::fs::write(segment, original).unwrap();
    assert_eq!(events(&recap(&a)).len(), 1);
    assert!(events(&recap(&a)).is_empty());
}

#[test]
fn malformed_provenance_is_reported_without_breaking_the_json_schema() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let bad = json!({
        "id": "01900000-0000-7000-8000-000000000001", "replica": "bad", "deps": [],
        "op": "comment", "actor": {"agent": 7}, "where": {"branch": 9},
        "data": {"text": "malformed provenance"}
    });
    let folder = a.store().join("events/bad");
    std::fs::create_dir(&folder).unwrap();
    let text = beadroll_core::json::write(&bad, beadroll_core::json::Format::COMPACT).unwrap();
    std::fs::write(folder.join("000001.jsonl"), text + "\n").unwrap();
    let answer = recap(&a);
    assert!(
        !answer["workspaces"][0]["integrity"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let rows = events(&answer);
    assert_eq!(rows.len(), 1);
    for key in ["agent", "thread", "name", "branch", "commit"] {
        assert_eq!(rows[0][key], Json::Null, "{key}");
    }
    assert!(!a.home().join(".recap.json").exists());
}

#[test]
fn empty_home_help_usage_and_hostile_text_keep_the_cli_contract() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    assert_eq!(
        a.cmd(["recap", "--no-advance"])
            .cwd(sandbox.root())
            .run()
            .stdout,
        "No local trackers\n"
    );
    assert!(!a.home().exists());
    assert_eq!(recap(&a)["workspaces"].as_array().unwrap().len(), 0);
    assert!(!a.home().join(".recap.json").exists());
    let help = a
        .cmd(["recap", "--help"])
        .cwd(sandbox.root())
        .env("PATH", "")
        .run();
    assert_eq!(help.returncode(), 0);
    assert!(help.stdout.contains("--no-advance"));
    let bad = a.run(["recap", "--unknown", "--json"]);
    assert_eq!(bad.returncode(), 2);
    assert_eq!(code(&bad), "USAGE");
    a.bead(["init", "--prefix", "ex"]);
    let title = "題名\u{1b}[31m\nforged\u{202e}";
    let key = create(&a, title);
    a.bead(["comments", "add", &key, "text\r\u{1b}]8;;bad\u{7}"]);
    let shown = a
        .cmd(["recap", "--no-advance"])
        .env("COLUMNS", "20")
        .env("NO_COLOR", "1")
        .run();
    assert_eq!(shown.returncode(), 0, "{}", shown.stderr);
    assert!(!shown.stdout.contains('\u{1b}'));
    assert!(!shown.stdout.contains('\r'));
    assert!(shown.stdout.contains("題名\\x1b[31m\\x0aforged\\u202e"));
    let json = a.run(["recap", "--no-advance", "--json", "--color", "always"]);
    assert!(json.stderr.is_empty());
    assert!(!json.stdout.contains('\u{1b}'));
    assert_eq!(
        events(&parse_json(&json.stdout).unwrap())[0]["title"].as_str(),
        Some(title)
    );
}
