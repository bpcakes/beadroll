//! An issue's id (`uid`, its create's event id): named beside its key in what every command
//! answers of it, the same on every machine and through every rename of its key, and taken
//! wherever a key is.

use beadroll_test_support::{Json, Machine, code, json_id};

/// The `uid` member of `answer`.
fn uid_of(answer: &Json) -> String {
    answer["uid"].as_str().expect("a uid").to_owned()
}

/// Each listed issue's key, by its uid, as `machine` lists every issue.
fn keys_by_uid(machine: &Machine) -> std::collections::BTreeMap<String, String> {
    let listed = machine.bead(["list", "--all", "--limit", "0"]).unwrap();
    (listed["issues"].as_array().unwrap().iter())
        .map(|issue| (uid_of(issue), json_id(issue)))
        .collect()
}

/// Two children made offline under one parent take one key, and a sync renumbers one: the uid each
/// create answered names the same issue on both machines after it, by `show` and by `list`, though
/// one key it answered names the other issue now.
#[test]
fn an_issues_uid_names_it_on_every_machine_whatever_its_key_becomes() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let x = json_id(&a.bead(["create", "Issue X"]).unwrap());
    a.bead(["sync"]);
    let b = crate::rust_only(&sandbox, "b");
    b.bead(["sync"]);
    let made = |machine: &Machine, title: &str| {
        let made = (machine.bead(["create", title, "--parent", &x, "--offline"])).unwrap();
        (json_id(&made), uid_of(&made))
    };
    let (on_a, on_b) = (made(&a, "on a"), made(&b, "on b"));
    assert_eq!(on_a.0, on_b.0, "one key minted on both");
    for machine in [&a, &b, &a] {
        machine.bead(["sync"]);
    }
    let mut moved = 0;
    for machine in [&a, &b] {
        for ((key, uid), title) in [(&on_a, "on a"), (&on_b, "on b")] {
            let shown = machine.show(uid);
            assert_eq!(shown["title"].as_str(), Some(title), "{uid}: {shown:?}");
            assert_eq!(uid_of(&shown), *uid);
            moved += usize::from(json_id(&shown) != *key);
        }
    }
    assert_eq!(moved, 2, "one child renumbered, on both machines");
    assert_eq!(keys_by_uid(&a), keys_by_uid(&b));
}

/// Every command takes an issue's uid where it takes its key, and names the issues it answers of by
/// uid beside their keys: `show`'s parent, dependencies and dependents, `dep`, a comment, `log`; a
/// uid that names no issue (a comment's) is `ISSUE_NOT_FOUND`.
#[test]
fn every_command_takes_an_issues_uid_and_names_issues_by_it() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let parent = uid_of(&a.bead(["create", "Parent"]).unwrap());
    let child = uid_of(&a.bead(["create", "Child", "--parent", &parent]).unwrap());
    let other = uid_of(&a.bead(["create", "Other"]).unwrap());
    let shown = a.show(&child);
    assert_eq!(
        shown["parent_uid"].as_str(),
        Some(parent.as_str()),
        "{shown:?}"
    );
    let dependents = &a.show(&parent)["dependents"];
    assert_eq!(dependents[0]["uid"].as_str(), Some(child.as_str()));
    let added = a.bead(["dep", "add", &child, &other]).unwrap();
    assert_eq!(
        added["issue_uid"].as_str(),
        Some(child.as_str()),
        "{added:?}"
    );
    assert_eq!(added["depends_on_uid"].as_str(), Some(other.as_str()));
    let dependencies = &a.show(&child)["dependencies"];
    let named: Vec<&str> = (dependencies.as_array().unwrap().iter())
        .filter_map(|dep| dep["uid"].as_str())
        .collect();
    assert!(named.contains(&other.as_str()), "{dependencies:?}");
    let comment = a.bead(["comments", "add", &child, "by its uid"]).unwrap();
    assert_eq!(comment["issue_uid"].as_str(), Some(child.as_str()));
    a.bead(["update", &child, "--title", "Renamed by uid"]);
    assert_eq!(a.show(&child)["title"].as_str(), Some("Renamed by uid"));
    let logged = a.bead(["log", &child]).unwrap();
    let rows = logged.as_array().expect("log rows");
    assert!(
        (rows.iter()).all(|row| row["issue_uid"].as_str() == Some(child.as_str())),
        "{logged:?}"
    );
    let not_an_issue = comment["id"].as_str().expect("a comment's id");
    let run = a.run(["show", not_an_issue, "--json"]);
    assert_eq!(code(&run), "ISSUE_NOT_FOUND", "{}", run.stderr);
}
