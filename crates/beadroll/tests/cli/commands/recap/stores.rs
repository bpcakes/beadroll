//! The trackers a recap reads, every store under `BEAD_HOME` and no checkout, one that cannot be
//! read among them, and the cursor it keeps of them: a frontier for each, in a file of its own.

use std::collections::BTreeSet;

use beadroll_test_support::{Json, Machine, Sandbox, code, parse_json};

use super::{create, each, events, recap};

/// A machine of a sandbox of its own, made by `make`, whose stores go to `machine`'s home: a second
/// repository with a tracker of its own on the same machine.
fn beside(machine: &Machine, make: fn(&Sandbox, &str) -> Machine) -> (Sandbox, Machine) {
    let sandbox = crate::sandbox();
    let mut other = make(&sandbox, "b");
    let home = machine.home().as_os_str().to_owned();
    other.extra.insert("BEAD_HOME".into(), home);
    (sandbox, other)
}

#[test]
fn every_tracker_shows_from_anywhere_without_git_or_the_network() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "a");
    let (_other, b) = beside(&a, Machine::new);
    a.bead(["init", "--prefix", "one"]);
    b.bead(["init", "--prefix", "two"]);
    let one = create(&a, "First repository");
    let two = create(&b, "Second repository");
    let listed = std::fs::read_dir(a.home()).unwrap().count();
    let anywhere = || {
        a.cmd(["recap", "--advance"])
            .cwd(sandbox.root())
            .env("PATH", "")
            .env("BEAD_WORKSPACE", "ignored")
            .env(
                "BEAD_FAILPOINTS",
                "fetch.result=exit(91);publish.before=exit(92)",
            )
    };
    let answer = anywhere().json().unwrap();
    let ids: Vec<&str> = (answer["workspaces"].as_array().unwrap().iter())
        .map(|tracker| tracker["workspace"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), 2);
    assert!(ids[0] < ids[1], "trackers in workspace-id order");
    let keys: BTreeSet<&str> = each(&answer, "issue").into_iter().collect();
    assert_eq!(keys, BTreeSet::from([one.as_str(), two.as_str()]));
    // the cursor and its lock, and nothing else
    assert_eq!(std::fs::read_dir(a.home()).unwrap().count(), listed + 2);
    assert!(events(&anywhere().json().unwrap()).is_empty());
    // one cursor for the home, whichever checkout asks
    assert!(events(&recap(&b)).is_empty());
}

#[test]
fn a_tracker_first_seen_after_a_recap_shows_what_came_since() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let (_other, b) = beside(&a, crate::rust_only);
    a.bead(["init", "--prefix", "one"]);
    create(&a, "Seen");
    recap(&a);
    // a tracker synced here today, with a history written before the last recap
    let before = |args: &[&str]| {
        b.cmd(args)
            .env("BEAD_FAILPOINTS", "clock@always=offset(-2d)")
            .json()
    };
    before(&["init", "--prefix", "two"]);
    before(&["create", "Older than the last recap"]);
    create(&b, "Newer than the last recap");
    let shown = recap(&a);
    assert_eq!(each(&shown, "title"), ["Newer than the last recap"]);
    let trackers = shown["workspaces"].as_array().unwrap();
    let new = trackers.iter().find(|t| t["earlier"] == Json::from(1_i64));
    assert!(
        new.is_some_and(|t| t["since"].as_str().is_some()),
        "{shown:?}"
    );
    assert!(events(&recap(&a)).is_empty());
}

#[test]
fn a_tracker_that_cannot_be_read_is_reported_and_the_others_still_show() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let (_other, b) = beside(&a, Machine::new);
    a.bead(["init", "--prefix", "one"]);
    b.bead(["init", "--prefix", "two"]);
    create(&a, "In the damaged store");
    create(&b, "In the healthy store");
    recap(&a);
    create(&a, "Unseen behind the damage");
    create(&b, "Unseen and readable");
    let path = |m: &Machine| {
        m.bead(["info"]).unwrap()["path"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let (damaged, healthy) = (path(&a), path(&b));
    let mut written = super::segments(std::path::Path::new(&damaged));
    written.sort();
    let segment = written.pop().unwrap();
    let original = std::fs::read(&segment).unwrap();
    std::fs::remove_file(&segment).unwrap();
    std::fs::create_dir(&segment).unwrap();
    std::fs::write(std::path::Path::new(&healthy).join("meta.json"), "{").unwrap();
    std::fs::create_dir_all(a.home().join("old copy/events")).unwrap();
    let cursor = std::fs::read(a.home().join(".recap.json")).unwrap();
    let answer = recap(&a);
    assert_eq!(each(&answer, "title"), ["Unseen and readable"]);
    let trackers = answer["workspaces"].as_array().unwrap();
    let errors: Vec<&str> = (trackers.iter())
        .filter_map(|tracker| tracker["error"]["code"].as_str())
        .collect();
    assert_eq!(errors, ["IO", "CONFIG"], "{answer:?}");
    let told = a.run(["recap"]).stdout;
    assert!(told.contains("old copy\n  cannot be read: "), "{told}");
    assert!(told.contains("is not a workspace id"), "{told}");
    // the damaged store kept its mark: once repaired, what it holds unseen shows
    let marks = |text: &[u8]| parse_json(std::str::from_utf8(text).unwrap()).unwrap();
    let id = trackers
        .iter()
        .find(|t| t["error"]["code"] == Json::from("IO"));
    let id = id.unwrap()["workspace"].as_str().unwrap().to_owned();
    let now = std::fs::read(a.home().join(".recap.json")).unwrap();
    assert_eq!(
        marks(&now)["workspaces"][&*id],
        marks(&cursor)["workspaces"][&*id]
    );
    std::fs::remove_dir(&segment).unwrap();
    std::fs::write(&segment, original).unwrap();
    assert_eq!(each(&recap(&a), "title"), ["Unseen behind the damage"]);
}

#[test]
fn a_tracker_unreadable_when_first_found_is_owed_its_window_until_it_is_shown() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let (_other, b) = beside(&a, Machine::new);
    a.bead(["init", "--prefix", "one"]);
    create(&a, "Seen");
    recap(&a);
    b.bead(["init", "--prefix", "two"]);
    create(&b, "Written after the last recap");
    let store = b.bead(["info"]).unwrap()["path"]
        .as_str()
        .unwrap()
        .to_owned();
    let segment = super::segments(std::path::Path::new(&store)).pop().unwrap();
    let original = std::fs::read(&segment).unwrap();
    std::fs::remove_file(&segment).unwrap();
    std::fs::create_dir(&segment).unwrap();
    // the new tracker cannot be read while another moves the cursor on, past its event's time
    create(&a, "Moves the cursor on");
    assert_eq!(each(&recap(&a), "title"), ["Moves the cursor on"]);
    assert!(a.run(["recap"]).stdout.contains("cannot be read"));
    std::fs::remove_dir(&segment).unwrap();
    std::fs::write(&segment, original).unwrap();
    let repaired = recap(&a);
    assert_eq!(each(&repaired, "title"), ["Written after the last recap"]);
    assert!(events(&recap(&a)).is_empty());
}

#[test]
fn a_store_whose_journal_path_is_unusable_when_first_found_is_owed_its_window() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let (_other, b) = beside(&a, Machine::new);
    a.bead(["init", "--prefix", "one"]);
    create(&a, "Seen");
    recap(&a);
    b.bead(["init", "--prefix", "two"]);
    create(&b, "Written before the cursor moved");
    let store = b.bead(["info"]).unwrap()["path"]
        .as_str()
        .unwrap()
        .to_owned();
    // its journal path is no directory while another tracker moves the cursor on
    let journals = std::path::Path::new(&store).join("events");
    let held = journals.with_extension("held");
    std::fs::rename(&journals, &held).unwrap();
    std::fs::write(&journals, "").unwrap();
    create(&a, "Moves the cursor on");
    assert_eq!(each(&recap(&a), "title"), ["Moves the cursor on"]);
    let told = a.run(["recap"]).stdout;
    assert!(told.contains("is not a directory"), "{told}");
    std::fs::remove_file(&journals).unwrap();
    std::fs::rename(&held, &journals).unwrap();
    let repaired = recap(&a);
    assert_eq!(
        each(&repaired, "title"),
        ["Written before the cursor moved"]
    );
    assert!(events(&recap(&a)).is_empty());
}

/// A tracker never shown, one event of it two days old and one new, whose segment `held` (by
/// order) has not arrived at its first recap, so that nothing of it but at most its init is in
/// causal order: it stays owed its window, and once the segment arrives shows only its new event
/// and counts the old one.
fn owed_until_its_events_can_show(held: usize) {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let (_other, mut b) = beside(&a, crate::rust_only);
    b.extra.insert("BEAD_ROTATE_BYTES".into(), "1".into());
    a.bead(["init", "--prefix", "one"]);
    create(&a, "Seen");
    recap(&a);
    let before = |args: &[&str]| {
        b.cmd(args)
            .env("BEAD_FAILPOINTS", "clock@always=offset(-2d)")
            .json()
    };
    before(&["init", "--prefix", "two", "--offline"]);
    before(&["create", "Older than the last recap", "--offline"]);
    b.bead(["create", "Newer than the last recap", "--offline"]);
    let store = b.bead(["info"]).unwrap()["path"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut written = super::segments(std::path::Path::new(&store));
    written.sort();
    let segment = written.remove(held);
    let away = segment.with_extension("held");
    std::fs::rename(&segment, &away).unwrap();
    let waiting = recap(&a);
    assert!(events(&waiting).is_empty(), "{waiting:?}");
    let tracker = (waiting["workspaces"].as_array().unwrap().iter())
        .find(|tracker| tracker["pending"].as_i64() > Some(0));
    assert!(tracker.is_some(), "{waiting:?}");
    std::fs::rename(away, segment).unwrap();
    let arrived = recap(&a);
    assert_eq!(each(&arrived, "title"), ["Newer than the last recap"]);
    let new = (arrived["workspaces"].as_array().unwrap().iter())
        .find(|tracker| tracker["earlier"] == Json::from(1_i64));
    assert!(new.is_some(), "{arrived:?}");
    assert!(events(&recap(&a)).is_empty());
}

#[test]
fn a_tracker_whose_events_all_wait_for_its_init_stays_owed_its_window() {
    owed_until_its_events_can_show(0);
}

#[test]
fn a_tracker_with_only_its_init_in_causal_order_stays_owed_its_window() {
    owed_until_its_events_can_show(1);
}

#[test]
fn a_recap_that_only_owes_a_window_leaves_the_time_a_later_tracker_starts_at() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let (_other, b) = beside(&a, Machine::new);
    a.bead(["init", "--prefix", "one"]);
    create(&a, "Seen");
    recap(&a);
    // written elsewhere after that recap, and first synced here after the next
    let elsewhere = crate::sandbox();
    let c = Machine::new(&elsewhere, "c");
    c.bead(["init", "--prefix", "three"]);
    create(&c, "Written between the recaps");
    // a tracker with only its init: owed its window, it marks nothing seen
    b.bead(["init", "--prefix", "two"]);
    assert!(events(&recap(&a)).is_empty());
    let store = c.store();
    std::fs::rename(&store, a.home().join(store.file_name().unwrap())).unwrap();
    let synced = recap(&a);
    assert_eq!(each(&synced, "title"), ["Written between the recaps"]);
    assert!(events(&recap(&a)).is_empty());
}

#[test]
fn the_cursor_files_cannot_be_taken_for_a_store() {
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
    assert!(a.home().join(".recap.json").is_file());
    assert!(events(&recap(&a)).is_empty());
}

#[test]
fn a_cursor_that_is_not_one_is_reported_and_never_replaced() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    create(&a, "Unseen");
    let path = a.home().join(".recap.json");
    for bytes in [
        "{",
        "[]",
        r#"{"version":2,"at":null,"workspaces":{}}"#,
        r#"{"version":1,"at":-5,"workspaces":{}}"#,
        r#"{"version":1,"at":null,"workspaces":{"x":{"frontier":["not-an-id"]}}}"#,
        r#"{"version":1,"at":null,"workspaces":{"x":{"frontier":[],"since":5}}}"#,
        r#"{"version":1,"at":null,"workspaces":{"x":{"since":-5}}}"#,
        r#"{"version":1,"at":null,"workspaces":{"x":["01a14b4b-95d9-7c23-b4ee-7497eb80f4e6"]}}"#,
        r#"{"version":1,"at":null,"workspaces":{},"future":true}"#,
    ] {
        std::fs::write(&path, bytes).unwrap();
        for args in [&["recap", "--json"][..], &["recap", "--advance", "--json"]] {
            let run = a.run(args);
            assert_eq!(
                (run.returncode(), code(&run)),
                (1, "IO".to_owned()),
                "{bytes}"
            );
            assert!(run.stdout.is_empty());
            assert!(run.stderr.contains(".recap.json"), "{}", run.stderr);
            assert_eq!(std::fs::read_to_string(&path).unwrap(), bytes);
        }
    }
    std::fs::remove_file(path).unwrap();
    assert_eq!(events(&recap(&a)).len(), 1);
}

#[test]
fn the_cursor_keeps_a_frontier_however_long_the_history() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let key = create(&a, "Busy");
    for n in 0..30 {
        a.bead([
            "comments",
            "add",
            &key,
            &format!("comment {n}"),
            "--offline",
        ]);
    }
    assert_eq!(events(&recap(&a)).len(), 31);
    let cursor = std::fs::read_to_string(a.home().join(".recap.json")).unwrap();
    let cursor = parse_json(&cursor).unwrap();
    let marks = cursor["workspaces"].as_object().unwrap();
    let (_, mark) = marks.iter().next().unwrap();
    assert_eq!(mark.as_object().unwrap().len(), 1, "{cursor:?}");
    assert_eq!(mark["frontier"].as_array().unwrap().len(), 1, "{cursor:?}");
}
