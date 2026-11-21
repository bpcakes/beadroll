//! An issue's id (`uid`, its create's event id): named beside its key in what every command
//! answers of it, the same on every machine and through every rename of its key, and taken
//! wherever a key is, keys first.

use std::collections::BTreeMap;

use beadroll_test_support::events::{Event, id_at};
use beadroll_test_support::{
    Json, Machine, code, converge, json, json_id, json_lines, json_member, path_text,
};

use super::fixtures::record;

/// The `uid` member of `answer`.
fn uid_of(answer: &Json) -> String {
    json_member(answer, "uid")
        .as_str()
        .expect("a uid")
        .to_owned()
}

/// The `uid` of every document in the array `items`, in order.
fn uids_of(items: &Json) -> Vec<String> {
    (items.as_array().expect("an array").iter())
        .map(uid_of)
        .collect()
}

/// Each listed issue's key, by its uid, as `machine` lists every issue.
fn keys_by_uid(machine: &Machine) -> BTreeMap<String, String> {
    let listed = machine.bead(["list", "--all", "--limit", "0"]).unwrap();
    (listed["issues"].as_array().unwrap().iter())
        .map(|issue| (uid_of(issue), json_id(issue)))
        .collect()
}

/// Two children made offline under one parent take one key, and a sync renumbers one: the uid each
/// create answered names the same issue on both machines after it, by `show` and by `list --id`,
/// though one key it answered names the other issue now.
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
    converge(&[&a, &b]);
    let mut moved = 0;
    for machine in [&a, &b] {
        for ((key, uid), title) in [(&on_a, "on a"), (&on_b, "on b")] {
            let shown = machine.show(uid);
            assert_eq!(shown["title"].as_str(), Some(title), "{uid}: {shown:?}");
            assert_eq!(uid_of(&shown), *uid);
            let listed = machine.bead(["list", "--all", "--id", uid]).unwrap();
            assert_eq!(uids_of(&listed["issues"]), [uid.as_str()]);
            assert_eq!(listed["issues"][0]["title"].as_str(), Some(title));
            moved += usize::from(json_id(&shown) != *key);
        }
    }
    assert_eq!(moved, 2, "one child renumbered, on both machines");
    assert_eq!(keys_by_uid(&a), keys_by_uid(&b));
}

/// `show`, `dep` (`list`, `tree`) and `comments` take an issue's uid where they take its key, and
/// name the issues they answer of by uid beside their keys (`parent_uid`, `parent_uids`,
/// dependencies, dependents and tree nodes); a uid that names no issue, or an uppercase UUID, is
/// `ISSUE_NOT_FOUND`.
#[test]
fn detail_and_graph_reads_take_an_issues_uid_and_name_issues_by_it() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let parent = uid_of(&a.bead(["create", "Parent"]).unwrap());
    let child = uid_of(&a.bead(["create", "Child", "--parent", &parent]).unwrap());
    let blocker = uid_of(&a.bead(["create", "Blocker"]).unwrap());
    a.bead(["dep", "add", &child, &blocker]);
    let comment = a.bead(["comments", "add", &child, "noted"]).unwrap();
    for args in [&["comments", &child][..], &["comments", "list", &child]] {
        assert_eq!(a.bead(args).unwrap().as_array().unwrap().len(), 1);
    }
    let second = uid_of(&a.bead(["create", "Second parent"]).unwrap());
    a.bead(["dep", "add", &child, &second, "--type", "parent-child"]);

    let shown = a.show(&child);
    assert_eq!(shown["parent_uid"].as_str(), Some(parent.as_str()));
    let parent_uids: Vec<&str> = (shown["parent_uids"].as_array().unwrap().iter())
        .filter_map(Json::as_str)
        .collect();
    assert!(parent_uids.contains(&parent.as_str()) && parent_uids.contains(&second.as_str()));
    assert!(uids_of(&shown["dependencies"]).contains(&blocker));
    assert!(uids_of(&a.show(&parent)["dependents"]).contains(&child));

    for args in [&["dep", &child][..], &["dep", "list", &child]] {
        let summary = a.bead(args).unwrap();
        assert!(uids_of(&summary["dependencies"]).contains(&blocker));
    }
    assert!(uids_of(&a.bead(["dep", &parent]).unwrap()["dependents"]).contains(&child));
    let tree = a.bead(["dep", "tree", &child]).unwrap();
    assert!(uids_of(&tree["nodes"]).contains(&blocker));

    let comment_id = comment["id"].as_str().expect("a comment's id");
    for bad in [comment_id, &parent.to_uppercase()] {
        let run = a.run(["show", bad, "--json"]);
        assert_eq!(code(&run), "ISSUE_NOT_FOUND", "{bad}: {}", run.stderr);
    }
}

/// `list` (`--id`, `--parent`), `ready` (root, `--epic`, `--parent`), `search`, `blocked`,
/// `claims`, `lint`, `log` and `recap` take an issue's uid where they take its key and name the
/// issues they list or record by uid (`uid`, `blocked_by_uids`, `issue_uid`).
#[test]
fn listing_and_activity_reads_take_an_issues_uid_and_name_issues_by_it() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let parent = uid_of(&a.bead(["create", "Parent"]).unwrap());
    let child = uid_of(&a.bead(["create", "Child", "--parent", &parent]).unwrap());
    let ready_leaf = uid_of(
        &a.bead(["create", "Ready leaf", "--parent", &parent])
            .unwrap(),
    );
    let blocker = uid_of(&a.bead(["create", "Blocker keyword"]).unwrap());
    a.bead(["dep", "add", &child, &blocker]);

    let by_id = format!("{child},{blocker}");
    let listed = uids_of(&a.bead(["list", "--id", &by_id]).unwrap()["issues"]);
    assert_eq!(listed.len(), 2);
    assert!(listed.contains(&child) && listed.contains(&blocker));
    let missing_id = "01900000-0000-7000-8000-000000000000";
    assert_eq!(
        a.bead(["list", "--id", missing_id]).unwrap()["total"],
        json!(0)
    );
    assert!(uids_of(&a.bead(["list", "--parent", &parent]).unwrap()["issues"]).contains(&child));
    for args in [
        &["ready", &parent][..],
        &["ready", "--epic", &parent],
        &["ready", "--parent", &parent],
    ] {
        assert_eq!(uids_of(&a.bead(args).unwrap()), [ready_leaf.as_str()]);
    }
    assert_eq!(
        uids_of(&a.bead(["search", "keyword"]).unwrap()),
        [blocker.as_str()]
    );

    let blocked = a.bead(["blocked"]).unwrap();
    let held = (blocked.as_array().unwrap().iter())
        .find(|item| item["uid"].as_str() == Some(child.as_str()))
        .expect("the child blocked");
    assert_eq!(held["blocked_by_uids"], json!([(blocker.as_str())]));

    a.bead(["claim", &blocker]);
    a.bead(["claim", &ready_leaf, "--offline"]);
    let claims = uids_of(&a.bead(["claims"]).unwrap()["claims"]);
    assert!(claims.contains(&blocker) && claims.contains(&ready_leaf));
    assert!(uids_of(&a.bead(["lint"]).unwrap()["findings"]).contains(&parent));

    let logged = a.bead(["log", &child]).unwrap();
    assert!(
        (logged.as_array().unwrap().iter())
            .all(|r| r["issue_uid"].as_str() == Some(child.as_str()))
    );
    let recapped = &a.bead(["recap", "--all"]).unwrap()["workspaces"][0]["events"];
    assert!((recapped.as_array().unwrap().iter()).all(|r| r["issue_uid"].as_str().is_some()));
}

/// Every command that writes an issue takes its uid where it takes its key, and names the issues
/// it answers of by uid: `create` (`--parent`, `--deps`, `--silent`), `dep` (`add`, `remove`),
/// `comments add`, `update` (with `--parent` and `--claim`), `claim`, `release`, `takeover`,
/// `close` (`--recursive`), `reopen` and `delete`.
#[test]
fn every_writing_command_takes_an_issues_uid_and_names_issues_by_it() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "a");
    let b = crate::rust_only(&sandbox, "b");
    a.bead(["init", "--prefix", "ex"]);
    let first = uid_of(&a.bead(["create", "First"]).unwrap());
    let silent = a.bead(["create", "Silent", "--silent"]).unwrap();
    let second = uid_of(&silent);
    assert_eq!(second, uid_of(&a.show(&json_id(&silent))));
    let child = a
        .bead(["create", "Child", "--parent", &first, "--deps", &second])
        .unwrap();
    let child = uid_of(&child);
    assert_eq!(a.show(&child)["parent_uid"].as_str(), Some(first.as_str()));

    let removed = a.bead(["dep", "remove", &child, &second]).unwrap();
    assert_eq!(removed["issue_uid"].as_str(), Some(child.as_str()));
    assert_eq!(removed["depends_on_uid"].as_str(), Some(second.as_str()));
    let added = a.bead(["dep", "add", &first, &second]).unwrap();
    assert_eq!(added["issue_uid"].as_str(), Some(first.as_str()));
    assert_eq!(added["depends_on_uid"].as_str(), Some(second.as_str()));

    let noted = a.bead(["comments", "add", &child, "by uid"]).unwrap();
    assert_eq!(noted["issue_uid"].as_str(), Some(child.as_str()));

    let updated = a
        .bead(["update", &child, "--parent", &second, "--claim"])
        .unwrap();
    assert_eq!(uid_of(&updated[0]), child);
    assert_eq!(updated[0]["parent_uid"].as_str(), Some(second.as_str()));
    let released = a.bead(["release", &child]).unwrap();
    assert_eq!(uid_of(&released[0]), child);

    let claimed = a.bead(["claim", &first]).unwrap();
    assert_eq!(uid_of(&claimed[0]), first);
    b.bead(["sync"]);
    let taken = b.bead(["takeover", &first, "-r", "handing over"]).unwrap();
    assert_eq!(uid_of(&taken[0]), first);
    a.bead(["sync"]);

    let closed = b
        .bead(["close", "--recursive", &second, "-r", "done"])
        .unwrap();
    let closed_uids = uids_of(&closed);
    assert!(closed_uids.contains(&second) && closed_uids.contains(&child));
    let reopened = b.bead(["reopen", &child]).unwrap();
    assert_eq!(uid_of(&reopened[0]), child);
    let deleted = b.bead(["delete", &child, "-r", "gone"]).unwrap();
    assert_eq!(uid_of(&deleted[0]), child);
}

/// Keys win over uids of the same spelling, in `show`, `list --id` and `dep remove`'s awaited
/// keys alike; and an awaited key, an edge to text that names no issue, and an orphan event each
/// report `null` where no issue exists.
#[test]
fn keys_and_awaited_keys_win_over_a_uid_and_non_issues_have_null_uids() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let first = a.bead(["create", "First"]).unwrap();
    let u1 = uid_of(&first);
    let second = a.bead(["create", "Second"]).unwrap();
    let (second_key, u2) = (json_id(&second), uid_of(&second));

    let waiting_deps = json!([{
        "issue_id": "ex-waiting",
        "depends_on_id": (u2.as_str()),
        "type": "blocks",
    }]);
    let export = sandbox.records(
        "uid-keys.jsonl",
        &[
            record(&u1, "Keyed like first's uid", json!({})),
            record(
                "ex-waiting",
                "Waiting",
                json!({"dependencies": waiting_deps}),
            ),
        ],
    );
    a.bead(["import", path_text(&export)]);

    let by_show = a.show(&u1);
    assert_eq!(by_show["title"].as_str(), Some("Keyed like first's uid"));
    assert_ne!(uid_of(&by_show), u1);
    let by_list = a.bead(["list", "--all", "--id", &u1]).unwrap();
    assert_eq!(uids_of(&by_list["issues"]), [uid_of(&by_show)]);

    // `ex-waiting` both has a live edge to `second` (whose uid is `u2`) and waits for key `u2`:
    // `dep remove ex-waiting <u2>` takes back the awaited key first, leaving the live edge intact.
    a.bead(["dep", "add", "ex-waiting", &second_key]);
    let retracted = a.bead(["dep", "remove", "ex-waiting", &u2]).unwrap();
    assert_eq!(retracted["depends_on_id"].as_str(), Some(u2.as_str()));
    assert_eq!(retracted["depends_on_uid"], Json::Null);
    let remaining = a.show("ex-waiting");
    assert!(remaining.get("unresolved_dependencies").is_none());
    assert_eq!(uids_of(&remaining["dependencies"]), [u2.as_str()]);

    let waiting_uid = uid_of(&remaining);
    let frontier: Vec<String> = (a.bead(["info"]).unwrap()["frontier"].as_array().unwrap())
        .iter()
        .filter_map(Json::as_str)
        .map(str::to_owned)
        .collect();
    let heads: Vec<&str> = frontier.iter().map(String::as_str).collect();
    let e1 = id_at(1_800_000_000_000, 1);
    let text_dep = Event::new(&e1, "dep_add")
        .replica("fffffff1")
        .deps(&heads)
        .issue(&waiting_uid)
        .data(json!({"to": "not-an-id", "type": "related"}))
        .json();
    let orphan_target = id_at(1_800_000_000_001, 2);
    let orphan = Event::new(&id_at(1_800_000_000_002, 3), "comment")
        .replica("fffffff1")
        .deps(&[&e1])
        .issue(&orphan_target)
        .data(json!({"text": "no such issue"}))
        .json();
    let folder = a.store().join("events/fffffff1");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("000001.jsonl"), json_lines(&[text_dep, orphan])).unwrap();

    let deps = &a.show(&waiting_uid)["dependencies"];
    let unknown = (deps.as_array().unwrap().iter())
        .find(|dep| dep["id"].as_str() == Some("not-an-id"))
        .expect("the text dependency");
    assert_eq!(unknown["uid"], Json::Null);
    assert_eq!(unknown["status"].as_str(), Some("unknown"));

    let orphan_uid = |rows: &Json| {
        (rows.as_array().unwrap().iter())
            .find(|row| row["issue"].as_str() == Some(orphan_target.as_str()))
            .expect("the orphan event")["issue_uid"]
            .clone()
    };
    let logged = a.bead(["log", "--limit", "0"]).unwrap();
    assert_eq!(orphan_uid(&logged), Json::Null);
    let recapped = &a.bead(["recap", "--all"]).unwrap()["workspaces"][0]["events"];
    assert_eq!(orphan_uid(recapped), Json::Null);
}
