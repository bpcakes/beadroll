//! `BpTest`: the core behaviours, end to end.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::sync::Arc;

use beadroll_core::journal::Journal;
use beadroll_core::json::{Format, Value, write};
use beadroll_core::replay::replay;
use beadroll_store::{Settings, Workspace, load_journal, replica_id};
use beadroll_test_support::{Json, Machine, Sandbox};

/// Machine a owns an initialised tracker with issues X and Y; machine b has synced it.
fn two_machines(sandbox: &Sandbox) -> (Machine, Machine, String, String) {
    let a = Machine::new(sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let x = a.bead(["create", "Issue X", "-d", "original"]).unwrap()["id"].clone();
    let y = a.bead(["create", "Issue Y"]).unwrap()["id"].clone();
    a.bead(["sync"]);
    let b = Machine::new(sandbox, "b");
    b.bead(["sync"]);
    let text = |v: Json| v.as_str().unwrap().to_owned();
    (a, b, text(x), text(y))
}

/// Every machine syncs twice; then they must export the same records and show the same issues.
fn settle(machines: &[&Machine]) {
    for machine in machines.iter().chain(machines) {
        machine.bead(["sync"]);
    }
    let exports: Vec<String> = machines.iter().map(|m| m.export()).collect();
    assert!(
        exports.iter().all(|e| *e == exports[0]),
        "machines did not converge"
    );
    let views: Vec<String> = machines
        .iter()
        .map(|m| {
            let listed = m.bead(["list", "--all", "--limit", "0"]).unwrap();
            let ids: Vec<String> = listed["issues"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["id"].as_str().unwrap().to_owned())
                .collect();
            let shown = m
                .bead(std::iter::once("show".to_owned()).chain(ids))
                .unwrap();
            write(&shown, Format::SORTED_ASCII).unwrap()
        })
        .collect();
    assert!(
        views.iter().all(|v| *v == views[0]),
        "machines disagree on claims or conflicts"
    );
}

/// The settings of a `bead` run on `machine`, as the test reads its store in-process.
fn settings(machine: &Machine) -> Arc<Settings> {
    let vars: BTreeMap<OsString, OsString> = machine.env(None, "codex").into_iter().collect();
    let user_home = std::env::var_os("HOME").unwrap_or_default();
    Arc::new(
        Settings::from_process(
            vars,
            Some(machine.checkout().to_path_buf()),
            user_home,
            std::env::temp_dir(),
            &|_| None,
        )
        .unwrap(),
    )
}

/// What a reader sees of `journal`: the export and every issue in full, as sorted JSON.
fn view(journal: Journal) -> String {
    let st = replay(journal).unwrap();
    let mut issues: Vec<_> = st.issues().collect();
    issues.sort_by(|a, b| a.key().cmp(b.key()));
    let export = Value::Array(st.export_records().into_iter().map(Value::Object).collect());
    let details = Value::Array(issues.iter().map(|i| Value::Object(st.detail(i))).collect());
    write(&Value::Array(vec![export, details]), Format::SORTED_ASCII).unwrap()
}

/// Replay is the same under every delivery order of the machines' journals, each event delivered
/// twice; an event whose cause has not arrived waits, never treated as concurrent.
#[test]
fn replay_does_not_depend_on_delivery_order() {
    let sandbox = crate::sandbox();
    let (a, b, x, y) = two_machines(&sandbox);
    let c = Machine::new(&sandbox, "c");
    c.bead(["sync"]);
    a.bead(["update", &x, "--priority", "0", "--add-label", "one"]);
    b.bead([
        "update",
        &x,
        "--priority",
        "3",
        "--remove-label",
        "one",
        "--notes",
        "from b",
    ]);
    c.bead(["update", &x, "--priority", "4"]);
    c.bead(["create", "child", "--parent", &y]);
    b.bead(["create", "other child", "--parent", &y]);
    a.bead(["claim", &y]);
    b.bead(["claim", &y, "--offline"]);
    settle(&[&a, &b, &c]);
    a.bead(["update", &x, "--notes", "after seeing everything"]);
    a.bead(["sync"]);

    let settings = settings(&a);
    let store = a.store();
    let id = store.file_name().unwrap().to_str().unwrap();
    let ws = Workspace::new(settings.clone(), id, None, None, Default::default()).unwrap();
    let journal = load_journal(&ws).unwrap();
    let expected = view(journal.clone());
    let sequenced: Vec<_> = journal.order().values().flatten().copied().collect();
    let ownership: Vec<Value> = journal
        .events()
        .iter()
        .filter(|(id, _)| !sequenced.contains(id))
        .map(|(_, event)| event.value().clone())
        .collect();
    let mut replicas: Vec<String> = journal.order().keys().cloned().collect();
    let mut orders = Vec::new();
    permutations(&mut replicas, 0, &mut orders);
    assert_eq!(orders.len(), 6);
    for order in orders {
        let mut shuffled = Journal::new();
        for replica in order.iter().chain(&order) {
            // every event delivered twice
            for id in &journal.order()[replica] {
                let event = journal.event(*id).unwrap().value().clone();
                shuffled.add(event, replica, "", true);
            }
            for event in &ownership {
                if event["replica"].as_str() == Some(replica.as_str()) {
                    shuffled.add(event.clone(), replica, "", false);
                }
            }
        }
        assert_eq!(view(shuffled), expected);
    }
    let own = replica_id(&settings, true).unwrap().unwrap();
    let mut partial = Journal::new();
    for id in &journal.order()[&own] {
        partial.add(journal.event(*id).unwrap().value().clone(), &own, "", true);
    }
    let st = replay(partial).unwrap();
    assert!(!st.pending().is_empty());
    let waiting: Vec<String> = st
        .pending()
        .iter()
        .map(|id| write(journal.event(*id).unwrap().value(), Format::SPACED).unwrap())
        .collect();
    assert!(waiting.join(" ").contains("after seeing everything"));
}

/// Every order of `items[at..]` after `items[..at]`, in the order itertools.permutations gives.
fn permutations(items: &mut Vec<String>, at: usize, out: &mut Vec<Vec<String>>) {
    if at == items.len() {
        out.push(items.clone());
        return;
    }
    for next in at..items.len() {
        let chosen = items.remove(next);
        items.insert(at, chosen);
        permutations(items, at + 1, out);
        let chosen = items.remove(at);
        items.insert(next, chosen);
    }
}
