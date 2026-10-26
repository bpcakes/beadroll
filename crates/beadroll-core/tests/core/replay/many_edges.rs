//! One issue given more edges than an issue holds before they are indexed (roll-lqjj.21), in every
//! way an edge comes and goes: an import naming keys that have their issues and keys made later,
//! `dep add` of the same edges, of other kinds on the same targets and of references that name no
//! issue, with metadata, removals by target, of one kind and of any, and by key, and edges added
//! again. Replay, a projection taking the history a record at a time from several points, and a
//! projection read back from its bytes at those points and extended after answer alike: the edges
//! in the order first added, the events behind each, each addition's metadata and the export.
//! These suites build beadroll-core as bead does, so the edges are indexed past the bound bead
//! runs with, where core's own tests index them once an issue has two, and the bounds themselves
//! are checked here: past 16 edges, and the thirteenth change of edges read back.

use beadroll_core::id::EventId;
use beadroll_core::journal::Journal;
use beadroll_core::json::Object;
use beadroll_core::projection::{Extended, Projection, Record, decode_state, project};
use beadroll_core::replay::replay;
use beadroll_core::state::{Issue, State};
use beadroll_test_support::events::{Event, id};
use beadroll_test_support::{Json, json};

/// How many issues the hub's import names that have their issues when it is made, and that are
/// made after it.
const NAMED: usize = 12;

/// How many references that name no issue `dep add` gives the hub an edge on.
const REFERENCES: usize = 12;

/// The kind of the import's dependency on issue `n`: blocks and related in turn.
fn kind(n: usize) -> &'static str {
    if n.is_multiple_of(2) {
        "related"
    } else {
        "blocks"
    }
}

/// One machine's history, each event built on the one before it.
#[derive(Default)]
struct History(Vec<Json>);

impl History {
    /// Event number `n`, doing `op` on `issue`, or on itself; returns its id.
    fn add(&mut self, op: &str, issue: Option<&str>, data: Json) -> String {
        let n = self.0.len() as u64 + 1;
        let me = id(n);
        let mut event = Event::new(&me, op).data(data);
        if n > 1 {
            event = event.deps(&[&id(n - 1)]);
        }
        if let Some(issue) = issue {
            event = event.issue(issue);
        }
        self.0.push(event.json());
        me
    }

    /// The create of the issue under `key`, importing `dangling`; returns its uid.
    fn create(&mut self, key: &str, dangling: Vec<Json>) -> String {
        let mut data = Object::new();
        data.insert("key".into(), Json::from(key));
        data.insert("fields".into(), json!({"title": key}));
        if !dangling.is_empty() {
            data.insert("imported".into(), Json::from(true));
            data.insert("dangling".into(), Json::Array(dangling));
        }
        self.add("create", None, Json::from(data))
    }

    /// The journal of its first `held` events.
    fn journal(&self, held: usize) -> Journal {
        let mut journal = Journal::new();
        for event in &self.0[..held] {
            journal.add(event.clone(), "a", "", true);
        }
        journal
    }

    /// Its events after the first `held`, as records a projection takes in.
    fn after(&self, held: usize) -> Vec<Record> {
        (self.0[held..].iter())
            .map(|value| Record {
                value: value.clone(),
                replica: "a".into(),
                origin: String::new(),
                sequenced: true,
            })
            .collect()
    }
}

/// The history, and the uid of the hub.
fn history() -> (History, String) {
    let mut history = History::default();
    history.add("init", None, json!({"prefix": "ex"}));
    let mut uids = Vec::new();
    for n in 1..=NAMED {
        uids.push(history.create(&format!("ex-{n}"), Vec::new()));
    }
    let dependency = |n: usize| {
        let metadata = format!("{{\"n\":{n}}}");
        json!({"depends_on_id": (format!("ex-{n}")), "type": (kind(n)), "created_by": "importer",
               "metadata": (metadata)})
    };
    let mut named: Vec<Json> = (1..=2 * NAMED).map(dependency).collect();
    named.push(json!({"depends_on_id": "ex-never", "type": "blocks"}));
    let hub = history.create("ex-hub", named);
    for n in NAMED + 1..=2 * NAMED {
        uids.push(history.create(&format!("ex-{n}"), Vec::new()));
    }
    let add = |history: &mut History, to: &str, kind: Option<&str>, note: usize| {
        let mut data = Object::new();
        data.insert("to".into(), Json::from(to));
        data.insert("meta".into(), json!({"note": (format!("added {note}"))}));
        if let Some(kind) = kind {
            data.insert("type".into(), Json::from(kind));
        }
        history.add("dep_add", Some(&hub), Json::from(data));
    };
    // other kinds on the targets the import named first, and the import's own edges again
    for (n, uid) in uids.iter().enumerate().take(6) {
        add(&mut history, uid, Some("waits-for"), n);
    }
    for (n, uid) in uids.iter().enumerate().skip(NAMED).take(6) {
        add(&mut history, uid, Some(kind(n + 1)), n);
    }
    for n in 0..REFERENCES {
        add(&mut history, &format!("jira:OPS-{n}"), None, 100 + n);
    }
    let remove = |history: &mut History, data: Json| {
        history.add("dep_remove", Some(&hub), data);
    };
    remove(&mut history, json!({"to": (uids[0].as_str())}));
    remove(
        &mut history,
        json!({"to": (uids[1].as_str()), "type": "waits-for"}),
    );
    remove(&mut history, json!({"key": "ex-3"}));
    remove(&mut history, json!({"to": "jira:OPS-0"}));
    remove(&mut history, json!({"key": "ex-never", "type": "blocks"}));
    add(&mut history, &uids[0], Some("blocks"), 200);
    add(&mut history, "jira:OPS-0", Some("related"), 201);
    (history, hub)
}

/// The hub of `state`.
fn hub<'a>(state: &'a State, uid: &str) -> &'a Issue {
    state.issue(EventId::parse(uid).unwrap()).unwrap()
}

/// The hub's edges, its target's key and its kind each, and how many additions are behind each
/// edge it has had, removed ones included.
fn edges(state: &State, uid: &str) -> (Vec<String>, Vec<usize>) {
    let issue = hub(state, uid);
    let edges = (issue.deps().keys())
        .map(|edge| format!("{} {}", state.target_key(edge.to()), edge.kind()))
        .collect();
    let tags = issue.dep_tags().values().map(|tags| tags.len()).collect();
    (edges, tags)
}

/// Asserts that `state` answers what `replayed` does of the hub's edges and in its export.
fn alike(state: &State, replayed: &State, uid: &str, what: &str) {
    assert_eq!(edges(state, uid), edges(replayed, uid), "{what}");
    assert_eq!(
        hub(state, uid).dep_tags(),
        hub(replayed, uid).dep_tags(),
        "{what}"
    );
    assert_eq!(state.export_records(), replayed.export_records(), "{what}");
}

/// `projection` with `records` applied one at a time.
fn one_at_a_time(mut projection: Projection, records: Vec<Record>) -> Projection {
    for record in records {
        projection = match projection.extend(vec![record]).unwrap() {
            Extended::Applied(extended) => *extended,
            Extended::Replay => panic!("a record of one machine's journal declined"),
        };
    }
    projection
}

/// `projection` read back from its bytes, its texts written after the end of `file`, as a store
/// appends them to its text file, and every issue's texts read in.
fn read_back(projection: &Projection, file: &mut Vec<u8>) -> Projection {
    let (encoded, _) = projection.encode(file.len() as u64);
    file.extend_from_slice(&encoded.texts);
    let alone = decode_state(&encoded.state).unwrap();
    let mut decoded = Projection::decode(encoded).unwrap();
    let state = decoded.state_mut();
    // a state read alone has every edge and the events behind it
    for issue in state.issues() {
        let read = alone.issue(issue.uid()).unwrap();
        let ids = |issue: &Issue| -> Vec<Vec<EventId>> {
            (issue.dep_tags().values())
                .map(|tags| tags.iter().map(|(id, _)| *id).collect())
                .collect()
        };
        assert!(read.dep_tags().keys().eq(issue.dep_tags().keys()));
        assert_eq!(ids(read), ids(issue));
    }
    for (uid, place) in state.all_wanted_texts() {
        let at = usize::try_from(place.at).unwrap();
        let bytes = &file[at..][..place.len as usize];
        state.read_texts(uid, place, bytes).unwrap();
    }
    decoded
}

#[test]
fn one_issue_given_many_edges_every_way_answers_alike_replayed_extended_and_read_back() {
    let (history, uid) = history();
    let replayed = replay(history.journal(history.0.len())).unwrap();
    assert!(hub(&replayed, &uid).dep_tags().indexed());
    let (shown, tags) = edges(&replayed, &uid);
    let mut expected: Vec<String> = vec!["ex-1 blocks".into()];
    expected.extend(
        (2..=NAMED)
            .filter(|n| *n != 3)
            .map(|n| format!("ex-{n} {}", kind(n))),
    );
    expected.extend((NAMED + 1..=2 * NAMED).map(|n| format!("ex-{n} {}", kind(n))));
    expected.extend((3..=6).map(|n| format!("ex-{n} waits-for")));
    expected.extend((1..REFERENCES).map(|n| format!("jira:OPS-{n} blocks")));
    expected.push("jira:OPS-0 related".into());
    assert_eq!(shown, expected, "the edges left, in the order first added");
    // ex-2's own import, then the import's edges again, and the edges removed, which keep their
    // places
    assert_eq!(tags.len(), 2 * NAMED + 6 + REFERENCES + 1);
    assert_eq!(tags[1], 1, "ex-2 kept its import");
    assert_eq!(tags[2], 0, "ex-3's import removed by its key");
    assert_eq!(&tags[NAMED..NAMED + 6], [2; 6], "imported and added");
    assert_eq!(tags[2 * NAMED], 0, "ex-1 waits-for removed");
    assert_eq!(tags[2 * NAMED + 6], 0, "jira:OPS-0 blocks removed");
    assert!(
        hub(&replayed, &uid).dangling().is_empty(),
        "ex-never removed"
    );
    let exported = (replayed.export_records().into_iter())
        .find(|record| record["id"].as_str() == Some("ex-hub"))
        .unwrap();
    let dependencies = exported["dependencies"].as_array().unwrap();
    assert_eq!(dependencies.len(), shown.len());
    assert!(
        (dependencies.iter()).any(|dep| dep["metadata"].as_str() == Some("{\"n\":5}")),
        "{dependencies:?}"
    );
    // taken a record at a time, and read back and extended, from points before and after the
    // edges are indexed, before and after the import's keys come, and among the removals
    let creates = 1 + NAMED + 1 + NAMED;
    for held in [
        1,
        NAMED + 1,
        NAMED + 2,
        creates - 4,
        creates + 6,
        creates + 22,
        history.0.len() - 3,
    ] {
        let projection = project(history.journal(held)).unwrap();
        let what = format!("from event {held}");
        let extended = one_at_a_time(projection.clone(), history.after(held));
        alike(
            extended.state(),
            &replayed,
            &uid,
            &format!("{what}, a record at a time"),
        );
        let mut file = Vec::new();
        let records = history.after(held);
        let read = read_back(&projection, &mut file);
        let Extended::Applied(batch) = read.extend(records).unwrap() else {
            panic!("{what}: one machine's records declined");
        };
        alike(
            batch.state(),
            &replayed,
            &uid,
            &format!("{what}, read back, in a batch"),
        );
        let records = history.after(held);
        let stepped = one_at_a_time(read_back(&projection, &mut file), records);
        alike(
            stepped.state(),
            &replayed,
            &uid,
            &format!("{what}, read back, a record at a time"),
        );
        let again = read_back(&stepped, &mut file);
        alike(
            again.state(),
            &replayed,
            &uid,
            &format!("{what}, read back again"),
        );
    }
}

/// A history of a hub given `edges` edges by `dep add`, one event each, on issues of their own,
/// and the hub's uid.
fn added(edges: usize) -> (History, String) {
    let mut history = History::default();
    history.add("init", None, json!({"prefix": "ex"}));
    let hub = history.create("ex-hub", Vec::new());
    for n in 0..edges {
        let to = history.create(&format!("ex-{n}"), Vec::new());
        history.add("dep_add", Some(&hub), json!({"to": (to.as_str())}));
    }
    (history, hub)
}

#[test]
fn an_issues_edges_are_indexed_past_sixteen_and_read_back_ones_at_their_thirteenth_change() {
    // the bound bead runs with: the 17th edge is added by searching 16 in order, the 18th by
    // indexing 17
    for (edges, indexed) in [(16, false), (17, false), (18, true), (40, true)] {
        let (history, uid) = added(edges);
        let state = replay(history.journal(history.0.len())).unwrap();
        let issue = hub(&state, &uid);
        assert_eq!(issue.dep_tags().len(), edges);
        assert_eq!(issue.dep_tags().indexed(), indexed, "{edges} edges");
    }
    // read back from a projection, its edges are searched in order by the first twelve changes, as
    // a command makes a few, and indexed by the thirteenth
    let (history, uid) = added(30);
    let held = history.0.len() - 2 * 13;
    let mut file = Vec::new();
    let mut read = read_back(&project(history.journal(held)).unwrap(), &mut file);
    assert!(!hub(read.state(), &uid).dep_tags().indexed(), "read back");
    let records = history.after(held);
    for (change, pair) in records.chunks(2).enumerate() {
        // a create of a target, then its edge
        read = one_at_a_time(read, pair.to_vec());
        let indexed = hub(read.state(), &uid).dep_tags().indexed();
        assert_eq!(indexed, change == 12, "after change {}", change + 1);
    }
    let replayed = replay(history.journal(history.0.len())).unwrap();
    alike(
        read.state(),
        &replayed,
        &uid,
        "read back and changed thirteen times",
    );
}
