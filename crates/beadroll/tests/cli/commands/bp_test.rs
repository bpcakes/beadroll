//! `BpTest`: the core behaviours, end to end. Its tests are grouped by subject in the modules
//! below; the replay test and the helpers they share are here.

mod checkouts;
mod claims;
mod compat;
mod crashes;
mod graph;
mod integrity;
mod merges;
mod remotes;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use beadroll_core::id::EventId;
use beadroll_core::journal::JournalOrder;
use beadroll_core::journal::{Journal, read_segment};
use beadroll_core::json::{Format, Value, write};
use beadroll_core::replay::replay;
use beadroll_store::IdentityMode;
use beadroll_store::{
    Settings, Workspace, claim_files, load_journal, read_attempt, replica_id, segment_files,
};
use beadroll_test_support::{Json, Machine, Run, Sandbox};

/// Machine a owns an initialised tracker with issues X and Y; machine b has synced it.
pub(crate) fn two_machines(sandbox: &Sandbox) -> (Machine, Machine, String, String) {
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
pub(crate) fn settle(machines: &[&Machine]) {
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

/// The `id` of a record bead printed: an issue, or what `create` made.
pub(crate) fn id(record: &Json) -> String {
    match record["id"].as_str() {
        Some(id) => id.to_owned(),
        None => panic!("no id in {record:?}"),
    }
}

/// Who holds the claim on `issue`, as `show` prints it.
pub(crate) fn owner(issue: &Json) -> &str {
    let owner = issue["claim"]["owner"].as_str();
    owner.unwrap_or_else(|| panic!("no claim owner in {issue:?}"))
}

/// `[row[key] for row in rows]`: the member `key` of every row of the list `rows`.
pub(crate) fn column(rows: &Json, key: &str) -> Vec<Json> {
    let rows = rows
        .as_array()
        .unwrap_or_else(|| panic!("not a list: {rows:?}"));
    rows.iter().map(|row| row[key].clone()).collect()
}

/// The number of items in `list`.
pub(crate) fn length(list: &Json) -> usize {
    let items = list
        .as_array()
        .unwrap_or_else(|| panic!("not a list: {list:?}"));
    items.len()
}

/// The exit statuses of `runs`, in ascending order.
pub(crate) fn sorted_codes(runs: &[Run]) -> Vec<i32> {
    let mut codes: Vec<i32> = runs.iter().map(Run::returncode).collect();
    codes.sort_unstable();
    codes
}

/// `f` of every job, run on `workers` threads at once, in the order of the jobs: what
/// `list(ThreadPoolExecutor(workers).map(f, jobs))` gives. A job that panics fails the test.
pub(crate) fn pool_map<J, R, F>(workers: usize, jobs: &[J], f: F) -> Vec<R>
where
    J: Sync,
    R: Send,
    F: Fn(&J) -> R + Sync,
{
    let next = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        let threads: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let index = next.fetch_add(1, Ordering::SeqCst);
                        let Some(job) = jobs.get(index) else {
                            return done;
                        };
                        done.push((index, f(job)));
                    }
                })
            })
            .collect();
        let mut done: Vec<(usize, R)> = threads
            .into_iter()
            .flat_map(|thread| thread.join().expect("a job panicked"))
            .collect();
        done.sort_by_key(|(index, _)| *index);
        done.into_iter().map(|(_, result)| result).collect()
    })
}

/// The segments of the journal in `store`, `events/*/*.jsonl`, in order of their paths.
pub(crate) fn segments(store: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for replica in std::fs::read_dir(store.join("events")).unwrap() {
        let replica = replica.unwrap().path();
        if !replica.is_dir() {
            continue;
        }
        for entry in std::fs::read_dir(&replica).unwrap() {
            let entry = entry.unwrap();
            if entry.file_name().to_str().unwrap().ends_with(".jsonl") {
                found.push(entry.path());
            }
        }
    }
    found.sort();
    found
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
    let export = Value::Array(st.export_records().into_iter().map(Value::from).collect());
    let details = Value::Array(issues.iter().map(|i| Value::from(st.detail(i))).collect());
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
    // the records as the store holds them: a journal keeps only what replay reads of each
    let segments = segment_files(&ws).unwrap().into_iter();
    let mut written: BTreeMap<EventId, Value> = BTreeMap::new();
    for event in segments.flat_map(|path| read_segment(&std::fs::read(path).unwrap(), "", None)) {
        written.insert(
            EventId::parse(event["id"].as_str().unwrap()).unwrap(),
            event,
        );
    }
    let ownership: Vec<Value> = (claim_files(&ws).unwrap().iter())
        .flat_map(|path| read_attempt(path, None))
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
                shuffled.add(written[id].clone(), replica, "", JournalOrder::Sequenced);
            }
            for event in &ownership {
                if event["replica"].as_str() == Some(replica.as_str()) {
                    shuffled.add(event.clone(), replica, "", JournalOrder::Attempt);
                }
            }
        }
        assert_eq!(view(shuffled), expected);
    }
    let own = replica_id(&settings, IdentityMode::Create)
        .unwrap()
        .unwrap();
    let mut partial = Journal::new();
    for id in &journal.order()[&own] {
        partial.add(written[id].clone(), &own, "", JournalOrder::Sequenced);
    }
    let st = replay(partial).unwrap();
    assert!(!st.pending().is_empty());
    let waiting: Vec<String> = st
        .pending()
        .iter()
        .map(|id| write(&written[id], Format::SPACED).unwrap())
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
