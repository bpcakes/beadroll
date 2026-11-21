//! Children made offline under a parent that a merge then renumbers: each follows the parent its
//! create names by id, at every depth and whatever else was renamed from the key its
//! writer saw, so that an issue keyed `A.n` is always a child of `A`.

use beadroll_test_support::{Json, Machine, json, json_id, json_line};

use super::settle;

/// Machine a owns an initialised tracker with issues X and Y, and machine b has synced it, both
/// this build's: the follow and the report these tests check are this build's contract, which an
/// older bead (`BEAD_UNDER_TEST`) does not keep, writing no parent in a create.
fn two_machines(sandbox: &beadroll_test_support::Sandbox) -> (Machine, Machine, String, String) {
    let a = crate::rust_only(sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let x = json_id(&a.bead(["create", "Issue X", "-d", "original"]).unwrap());
    let y = json_id(&a.bead(["create", "Issue Y"]).unwrap());
    a.bead(["sync"]);
    let b = crate::rust_only(sandbox, "b");
    b.bead(["sync"]);
    (a, b, x, y)
}

/// The key every issue `machine` lists has, by title.
fn keys(machine: &Machine) -> std::collections::BTreeMap<String, String> {
    let listed = machine.bead(["list", "--all", "--limit", "0"]).unwrap();
    (listed["issues"].as_array().unwrap().iter())
        .map(|issue| (issue["title"].as_str().unwrap().to_owned(), json_id(issue)))
        .collect()
}

/// Checks on every one of `machines` that an issue keyed `A.n` is a child of `A`, and that all list
/// the same keys; the keys, by title.
fn keys_name_their_parents(machines: &[&Machine]) -> std::collections::BTreeMap<String, String> {
    let first = keys(machines[0]);
    for machine in machines {
        let listed = keys(machine);
        assert_eq!(listed, first, "the machines disagree");
        for key in listed.values() {
            let Some((parent, number)) = key.rsplit_once('.') else {
                continue;
            };
            if !number.bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            let shown = machine.show(key);
            assert_eq!(shown["parent"].as_str(), Some(parent), "{key}: {shown:?}");
        }
    }
    first
}

/// The key `machine` gives `title`, made offline under `parent`.
fn offline(machine: &Machine, title: &str, parent: &str) -> String {
    json_id(
        &machine
            .bead(["create", title, "--parent", parent, "--offline"])
            .unwrap(),
    )
}

/// A child made under a parent that a merge then renames follows it: `x.1.1`, made on b under b's
/// own `x.1`, which a's child of that number came first and made `x.2`, is `x.2.1`, and sync
/// reports both renames. It kept `x.1.1`, which read as a child of a's `x.1`, whose next child then
/// skipped a number.
#[test]
fn a_child_follows_its_parents_rename() {
    let sandbox = crate::sandbox();
    let (a, b, x, _) = two_machines(&sandbox);
    let first = format!("{x}.1");
    assert_eq!(
        json_id(&a.bead(["create", "on a", "--parent", &x]).unwrap()),
        first
    );
    let made = json_id(
        &b.bead(["create", "on b", "--parent", &x, "--offline"])
            .unwrap(),
    );
    let grand = json_id(
        &b.bead(["create", "under b's", "--parent", &made, "--offline"])
            .unwrap(),
    );
    assert_eq!(grand, format!("{first}.1"));
    let report = b.bead(["sync"]).unwrap();
    let renamed: Vec<(String, String)> = (report["renamed"].as_array().unwrap().iter())
        .map(|r| {
            (
                r["from"].as_str().unwrap().to_owned(),
                r["to"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let second = format!("{x}.2");
    assert_eq!(
        renamed,
        [
            (first.clone(), second.clone()),
            (format!("{first}.1"), format!("{second}.1"))
        ]
    );
    settle(&[&a, &b]);
    for machine in [&a, &b] {
        assert_eq!(
            machine.show(&format!("{second}.1"))["title"].as_str(),
            Some("under b's")
        );
        assert_eq!(
            machine.show(&format!("{second}.1"))["parent"].as_str(),
            Some(second.as_str())
        );
    }
    let next = json_id(&a.bead(["create", "next on a", "--parent", &first]).unwrap());
    assert_eq!(next, format!("{first}.1"), "a's child skipped a number");
}

/// A grandchild made under a child that follows its parent follows it in turn, at every depth:
/// b's `x.1`, `x.1.1` and `x.1.1.1`, made offline under one another while a's `x.1` came first,
/// are `x.2`, `x.2.1` and `x.2.1.1`. The grandchild kept `x.1.1.1` when a follow's cause was the
/// parent followed, which every writer of a grandchild has seen.
#[test]
fn a_grandchild_follows_its_parent_that_followed() {
    let sandbox = crate::sandbox();
    let (a, b, x, _) = two_machines(&sandbox);
    a.bead(["create", "A", "--parent", &x]);
    let mut parent = x.clone();
    for title in ["B", "C", "G"] {
        let made = b.bead(["create", title, "--parent", &parent, "--offline"]);
        parent = json_id(&made.unwrap());
    }
    assert_eq!(parent, format!("{x}.1.1.1"));
    settle(&[&a, &b]);
    for machine in [&a, &b] {
        let listed = machine.bead(["list", "--all", "--limit", "0"]).unwrap();
        let key_of = |title: &str| {
            (listed["issues"].as_array().unwrap().iter())
                .find(|issue| issue["title"] == Json::from(title))
                .map(json_id)
                .unwrap_or_else(|| panic!("no {title} in {listed:?}"))
        };
        let keys = ["B", "C", "G"].map(key_of);
        assert_eq!(
            keys,
            [".2", ".2.1", ".2.1.1"].map(|tail| format!("{x}{tail}"))
        );
        let shown = machine.show(&keys[2]);
        assert_eq!(
            shown["parent"].as_str(),
            Some(keys[1].as_str()),
            "{shown:?}"
        );
    }
}

/// A child follows the parent its writer meant when that writer saw two renames of one key: c saw
/// b's child as `x.1` and its own as `x.2`, made a child of `x.1`, and a's `x.1` then came first,
/// so b's became `x.2` and c's `x.3`. The child follows b's, its parent, to `x.2.1`, never c's.
#[test]
fn a_child_follows_the_parent_its_writer_meant_among_renames() {
    let sandbox = crate::sandbox();
    let (a, b, x, _) = two_machines(&sandbox);
    let c = crate::rust_only(&sandbox, "c");
    c.bead(["sync"]);
    a.bead(["create", "on a", "--parent", &x, "--offline"]);
    b.bead(["create", "on b", "--parent", &x, "--offline"]);
    b.bead(["sync"]);
    c.bead(["create", "on c", "--parent", &x, "--offline"]);
    c.bead(["sync"]);
    assert_eq!(c.show(&format!("{x}.1"))["title"].as_str(), Some("on b"));
    let under = ["create", "under b's", "--parent", &format!("{x}.1")];
    assert_eq!(json_id(&c.bead(under).unwrap()), format!("{x}.1.1"));
    settle(&[&a, &b, &c]);
    // b's child, by its title: its key is the one the merge gave it
    let listed = a.bead(["list", "--all", "--limit", "0"]).unwrap();
    let key_of = |title: &str| {
        (listed["issues"].as_array().unwrap().iter())
            .find(|issue| issue["title"] == Json::from(title))
            .map(json_id)
            .unwrap_or_else(|| panic!("no {title} in {listed:?}"))
    };
    let (parent, child) = (key_of("on b"), key_of("under b's"));
    assert_eq!(
        child,
        format!("{parent}.1"),
        "the child follows another sibling"
    );
    for machine in [&a, &b, &c] {
        let shown = machine.show(&child);
        assert_eq!(shown["parent"].as_str(), Some(parent.as_str()), "{shown:?}");
    }
}

/// A rename the writer saw made is no key its issue held for it. d saw A as `x.1`, b's B renamed
/// to `x.2` with B's child R following it (`x.1.1` → `x.2.1`), and its own S under A as `x.1.1`; a
/// child of `x.1.1` it made is S's. R's follow, first in replay order from `x.1.1`, was made in
/// d's view too (it saw B's rename), so the child does not follow R, but S, renamed to `x.1.2` once
/// c's child of A came first.
#[test]
fn a_child_does_not_follow_a_rename_its_writer_saw_made() {
    let sandbox = crate::sandbox();
    let (a, b, x, _) = two_machines(&sandbox);
    let (c, d) = (
        crate::rust_only(&sandbox, "c"),
        crate::rust_only(&sandbox, "d"),
    );
    let titled = |machine: &Machine, title: &str| {
        let listed = machine.bead(["list", "--all", "--limit", "0"]).unwrap();
        (listed["issues"].as_array().unwrap().iter())
            .find(|issue| issue["title"] == Json::from(title))
            .map(json_id)
            .unwrap_or_else(|| panic!("no {title} in {listed:?}"))
    };
    a.bead(["create", "A", "--parent", &x]);
    let on_b = json_id(
        &b.bead(["create", "B", "--parent", &x, "--offline"])
            .unwrap(),
    );
    b.bead(["create", "R", "--parent", &on_b, "--offline"]);
    c.bead(["sync"]);
    d.bead(["sync"]);
    let a_key = format!("{x}.1");
    c.bead(["create", "H", "--parent", &a_key, "--offline"]);
    d.bead(["create", "S", "--parent", &a_key, "--offline"]);
    b.bead(["sync"]);
    d.bead(["sync"]);
    assert_eq!(titled(&d, "R"), format!("{x}.2.1"));
    assert_eq!(titled(&d, "S"), format!("{x}.1.1"));
    let under = ["create", "W", "--parent", &format!("{x}.1.1"), "--offline"];
    d.bead(under);
    c.bead(["sync"]);
    d.bead(["sync"]);
    settle(&[&a, &b, &c, &d]);
    for machine in [&a, &b, &c, &d] {
        let (s_key, w_key) = (titled(machine, "S"), titled(machine, "W"));
        assert_eq!(
            w_key,
            format!("{s_key}.1"),
            "W follows another issue than S"
        );
        let shown = machine.show(&w_key);
        assert_eq!(shown["parent"].as_str(), Some(s_key.as_str()), "{shown:?}");
    }
}

/// A writer that made two children and a grandchild under the second, before another machine's
/// child of the same number came first: all three move up (`x.2` and `x.3`), and the grandchild,
/// minted under `x.2`, follows its parent to `x.3.1`. It kept `x.2.1`, under the first child's key,
/// when the writer had seen the issue holding `x.2` now (under another key). (AT100)
#[test]
fn a_grandchild_follows_its_parent_past_a_key_its_writer_saw_held_otherwise() {
    let sandbox = crate::sandbox();
    let (a, c, x, _) = two_machines(&sandbox);
    offline(&c, "C", &x);
    offline(&a, "A1", &x);
    let second = offline(&a, "A2", &x);
    offline(&a, "G", &second);
    settle(&[&a, &c]);
    let keys = keys_name_their_parents(&[&a, &c]);
    assert_eq!(keys["G"], format!("{}.1", keys["A2"]));
}

/// A child made under a key a merge gave its parent: b saw B renamed to `x.2` and made G under it;
/// a third machine's earlier child then moves B to `x.3`, and G follows to `x.3.1`, where it kept
/// `x.2.1`, under the issue holding `x.2` now. (AT100)
#[test]
fn a_child_follows_a_parent_renamed_twice() {
    let sandbox = crate::sandbox();
    let (a, b, x, _) = two_machines(&sandbox);
    let c = crate::rust_only(&sandbox, "c");
    c.bead(["sync"]);
    offline(&c, "Z", &x);
    a.bead(["create", "H", "--parent", &x]);
    offline(&b, "B", &x);
    b.bead(["sync"]);
    let renamed = keys(&b)["B"].clone();
    assert_eq!(renamed, format!("{x}.2"));
    offline(&b, "G", &renamed);
    settle(&[&a, &b, &c]);
    let keys = keys_name_their_parents(&[&a, &b, &c]);
    assert_eq!(keys["G"], format!("{}.1", keys["B"]));
}

/// A grandchild under a child of an issue a merge renamed, while another subtree was renamed from
/// the key the writer saw: d's C under H1 (`x.1.1`) and G under C, then b's earlier child moves
/// H1 to `x.2`; C follows to `x.2.1` and G to `x.2.1.1`, not into the subtree of S1, which another
/// rename took from `x.1.1`. (AT100)
#[test]
fn a_grandchild_follows_its_own_subtree_among_renames_of_its_key() {
    let sandbox = crate::sandbox();
    let (b, c, x, _) = two_machines(&sandbox);
    let d = crate::rust_only(&sandbox, "d");
    d.bead(["sync"]);
    offline(&b, "H0", &x);
    offline(&c, "H1", &x);
    c.bead(["sync"]);
    let s = offline(&d, "S", &x);
    offline(&d, "S1", &s);
    d.bead(["sync"]);
    let seen = keys(&d);
    let child = offline(&d, "C", &seen["H1"]);
    offline(&d, "G", &child);
    b.bead(["sync"]);
    settle(&[&b, &c, &d]);
    let keys = keys_name_their_parents(&[&b, &c, &d]);
    assert_eq!(keys["C"], format!("{}.1", keys["H1"]));
    assert_eq!(keys["G"], format!("{}.1", keys["C"]));
}

/// A sync reports every key its merge changed, a key an earlier sync had renamed already among
/// them: b's B and its child C, renamed to `x.2` and `x.2.1` by b's first sync, move to `x.3` and
/// `x.3.1` once a's earlier child arrives, and b's next sync reports both, from the keys they had
/// before it. It reported the renames past the count it had before, which a rename made again does
/// not add to. (AT101)
#[test]
fn a_sync_reports_a_key_renamed_again() {
    let sandbox = crate::sandbox();
    let (a, b, x, _) = two_machines(&sandbox);
    let c = crate::rust_only(&sandbox, "c");
    c.bead(["sync"]);
    offline(&a, "A1", &x);
    c.bead(["create", "H", "--parent", &x]);
    let made = offline(&b, "B", &x);
    offline(&b, "C", &made);
    b.bead(["sync"]);
    let had = keys(&b);
    assert_eq!(had["C"], format!("{x}.2.1"));
    a.bead(["sync"]);
    let report = b.bead(["sync"]).unwrap();
    let reported: Vec<(String, String)> = (report["renamed"].as_array().unwrap().iter())
        .map(|r| {
            (
                r["from"].as_str().unwrap().to_owned(),
                r["to"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let now = keys(&b);
    for title in ["B", "C"] {
        // from the key the issue had before this sync, which the user saw last
        let renamed = (had[title].clone(), now[title].clone());
        assert!(
            reported.contains(&renamed),
            "{title}: {renamed:?} not in {reported:?}"
        );
    }
    assert_eq!(now["C"], format!("{}.1", now["B"]));
}

/// A sync reports a key an issue holds again, its rename gone: the import I, keyed `x.2.1`, gave
/// way to C, which followed B into `x.2.1`, and holds `x.2.1` again once a's earlier child moves B
/// to `x.3` and C to `x.3.1`. Reporting the renames that remained, the sync left I out. (AT101)
#[test]
fn a_sync_reports_a_key_held_again() {
    let sandbox = crate::sandbox();
    let (a, b, x, _) = two_machines(&sandbox);
    let (c, d) = (
        crate::rust_only(&sandbox, "c"),
        crate::rust_only(&sandbox, "d"),
    );
    c.bead(["sync"]);
    d.bead(["sync"]);
    offline(&a, "A1", &x);
    c.bead(["create", "H", "--parent", &x]);
    let made = offline(&b, "B", &x);
    offline(&b, "C", &made);
    d.bead(["sync"]);
    let imported = format!("{x}.2.1");
    let file = sandbox.root().join("i.jsonl");
    let id = imported.clone();
    let record = json!({
        "id": id, "title": "I", "status": "open", "priority": 2, "issue_type": "task",
        "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
    });
    std::fs::write(&file, json_line(&record)).unwrap();
    d.bead(["import", file.to_str().unwrap(), "--offline"]);
    b.bead(["sync"]);
    d.bead(["sync"]);
    assert_eq!(
        keys(&d)["I"],
        format!("{x}.2.2"),
        "C did not displace the import"
    );
    a.bead(["sync"]);
    let report = d.bead(["sync"]).unwrap();
    assert_eq!(keys(&d)["I"], imported);
    let reported: Vec<(String, String)> = (report["renamed"].as_array().unwrap().iter())
        .map(|r| {
            (
                r["from"].as_str().unwrap().to_owned(),
                r["to"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let back = (format!("{x}.2.2"), imported);
    assert!(reported.contains(&back), "{back:?} not in {reported:?}");
}

/// A child's create names its parent among its causes as well as in its data: replay follows only
/// a parent the child's writer saw, and the heads of the store need not reach it, nor keep a path
/// to it (an event a foreign journal's pending cycle names is on no head's path, and a foreign
/// journal put back as published can take one away). Here the parent is no head, Q made after it
/// (AT100).
#[test]
fn a_childs_create_names_its_parent_among_its_causes() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let parent = json_id(&a.bead(["create", "P"]).unwrap());
    let q_key = json_id(&a.bead(["create", "Q"]).unwrap());
    a.bead(["create", "C", "--parent", &parent, "--offline"]);
    let mut events = Vec::new();
    for entry in walk(&a.store().join("events")) {
        if entry.extension().is_some_and(|ext| ext == "jsonl") {
            let text = std::fs::read_to_string(&entry).unwrap();
            events.extend(
                text.lines()
                    .map(|line| beadroll_test_support::parse_json(line).unwrap()),
            );
        }
    }
    let create_of = |key: &str| {
        (events.iter())
            .find(|e| e["op"] == Json::from("create") && e["data"]["key"] == Json::from(key))
            .unwrap_or_else(|| panic!("no create of {key}"))
            .clone()
    };
    let parent_id = create_of(&parent)["id"].clone();
    let child = create_of(&format!("{parent}.1"));
    assert_eq!(child["data"]["parent"], parent_id);
    let q = create_of(&q_key);
    assert_eq!(child["deps"], Json::Array(vec![q["id"].clone(), parent_id]));
}

/// Every file under `dir`, at any depth.
fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(walk(&path));
        } else {
            files.push(path);
        }
    }
    files
}

/// A child's create a peer relays follows its parent's id on every upgraded machine: b, whichever
/// bead `BEAD_UNDER_TEST` names (this build's by default; an older peer only when it names a
/// released one, which ignores `parent`), relays a's child of a renumbered parent and publishes a
/// write of its own after it, and alone to a second remote, which a fresh machine c of this build
/// syncs from; c, and a, show the child under its parent's key now and its parent edge naming it
/// (AT100).
#[test]
fn a_child_follows_its_parent_after_a_peer_relays_it() {
    let sandbox = crate::sandbox();
    let (a, b, x, _) = two_machines(&sandbox);
    let older = Machine::new(&sandbox, "older");
    older.bead(["sync"]);
    // Q, made first, holds `x.1`: a's P, minted `x.1` offline after it, gives way, and its child C
    // follows it
    older.bead(["create", "Q", "--parent", &x]);
    let parent = offline(&a, "P", &x);
    offline(&a, "C", &parent);
    a.bead(["sync"]);
    assert_eq!(keys(&a)["P"], format!("{x}.2"), "P gave way to Q");
    older.bead(["sync"]);
    let relayed = keys(&older)["C"].clone();
    older.bead(["comments", "add", &relayed, "relayed by an older peer"]);
    older.bead(["sync"]);
    b.bead(["sync"]);
    // c syncs from a remote only the older peer publishes to: what it holds of the child, the
    // older peer's journal copied a's create into
    let relay = sandbox.root().join("relay.git");
    beadroll_test_support::git(["init", "-q", "--bare", relay.to_str().unwrap()], None);
    let relay = relay.to_str().unwrap();
    older.bead(["sync", "--remote", relay]);
    let c = crate::rust_only(&sandbox, "c");
    c.bead(["sync", "--remote", relay]);
    // what the older peer published, before a syncs again: the child under its parent, with the
    // comment it wrote on it
    let fresh = keys(&c);
    let shown = c.show(&fresh["C"]);
    assert_eq!(
        shown["parent"].as_str(),
        Some(fresh["P"].as_str()),
        "{shown:?}"
    );
    let texts: Vec<&str> = (shown["comments"].as_array().expect("its comments").iter())
        .filter_map(|comment| comment["text"].as_str())
        .collect();
    assert_eq!(texts, ["relayed by an older peer"], "{shown:?}");
    for machine in [&a, &b, &c] {
        machine.bead(["sync"]);
        let now = keys(machine);
        assert_eq!(now["C"], format!("{}.1", now["P"]), "{now:?}");
        let shown = machine.show(&now["C"]);
        assert_eq!(
            shown["parent"].as_str(),
            Some(now["P"].as_str()),
            "{shown:?}"
        );
    }
}
