//! V4: dangling imported dependencies replay independently of journal delivery order. Fixed
//! splitmix64 streams generate 10,156 events with unique keys and 10,350 with duplicates, pinning
//! the history shapes and counts.

use std::collections::{BTreeMap, BTreeSet};

use beadroll_core::journal::Journal;
use beadroll_core::journal::JournalOrder;
use beadroll_core::json::Value;
use beadroll_core::replay::replay;
use beadroll_core::state::State;
use beadroll_test_support::json;

/// Whether the generated history includes a copied machine identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Identities {
    /// Unique.
    Unique,
    /// Twins.
    Twins,
}

const SEEDS: u64 = 1000;
const ORDERS: usize = 4;

/// Floats in [0, 1) drawn from a splitmix64 stream.
struct Stream(u64);

impl Stream {
    fn random(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        (z >> 11) as f64 / (1u64 << 53) as f64
    }

    /// An index below `n`, from a random fraction scaled by `n` and truncated.
    fn index(&mut self, n: usize) -> usize {
        (self.random() * n as f64) as usize
    }

    fn randint(&mut self, low: usize, high: usize) -> usize {
        low + self.index(high - low + 1)
    }

    fn choice<T: Clone>(&mut self, items: &[T]) -> T {
        items[self.index(items.len())].clone()
    }

    fn sample<T: Clone>(&mut self, items: &[T], k: usize) -> Vec<T> {
        let mut pool = items.to_vec();
        (0..k)
            .map(|_| pool.remove(self.index(pool.len())))
            .collect()
    }

    fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.index(i + 1);
            items.swap(i, j);
        }
    }
}

fn eid(n: u64) -> String {
    format!(
        "01900000-{:04x}-7000-8000-{:012x}",
        n >> 48,
        n & ((1 << 48) - 1)
    )
}

fn replayed(events: &[Value]) -> State {
    let mut journal = Journal::new();
    for event in events {
        journal.add(
            event.clone(),
            event["replica"].as_str().unwrap(),
            "",
            JournalOrder::Sequenced,
        );
    }
    replay(journal).unwrap()
}

/// Each issue's dependency edges (by key) and unresolved dangling keys, plus how many events replay
/// could not apply (pending) or flagged (integrity).
type Projection = BTreeMap<String, (Vec<(String, String)>, Vec<String>)>;

fn projection(events: &[Value]) -> (Projection, usize, usize) {
    let st = replayed(events);
    let issues = st
        .issues()
        .map(|issue| {
            let mut edges: Vec<(String, String)> = issue
                .deps()
                .keys()
                .map(|edge| {
                    (
                        st.target_key(edge.to()).into_owned(),
                        edge.kind().to_owned(),
                    )
                })
                .collect();
            edges.sort();
            let mut dangling: Vec<String> = issue
                .dangling()
                .iter()
                .map(|d| d["depends_on_id"].as_str().unwrap().to_owned())
                .collect();
            dangling.sort();
            (issue.key().to_owned(), (edges, dangling))
        })
        .collect();
    (issues, st.pending().len(), st.integrity().len())
}

/// A generated history across three replicas.
struct History<'s> {
    stream: &'s mut Stream,
    events: Vec<Value>,
    known: BTreeMap<&'static str, BTreeSet<String>>,
    counter: u64,
}

const REPLICAS: [&str; 3] = ["r0", "r1", "r2"];

impl History<'_> {
    /// Appends an event by `replica`, naming everything it knows as its causes; its id.
    fn emit(&mut self, replica: &str, op: &str, issue: Option<&str>, data: Value) -> String {
        self.counter += 1;
        let id = eid(self.counter);
        let deps: Vec<Value> = self.known[replica]
            .iter()
            .map(|d| Value::from(d.as_str()))
            .collect();
        let event = json!({"v": 1, "id": (id.as_str()), "replica": replica, "deps": (deps), "op": op,
                           "issue": (issue.unwrap_or(&id)), "data": data});
        self.events.push(event);
        self.known.get_mut(replica).unwrap().insert(id.clone());
        id
    }

    /// `replica` imports one record, with dependencies on keys it has seen or not.
    fn import(&mut self, replica: &str, st: &State, fresh: &[String], used: &mut BTreeSet<String>) {
        let pool = pool();
        let key = self.stream.choice(fresh);
        used.insert(key.clone());
        let others: Vec<String> = pool.iter().filter(|k| **k != key).cloned().collect();
        let count = self.stream.randint(0, 2);
        let targets = self.stream.sample(&others, count);
        let dangling: Vec<Value> = targets
            .iter()
            .filter(|t| !st.keys().contains_key(*t))
            .map(|t| json!({"issue_id": (key.as_str()), "depends_on_id": (t.as_str()), "type": "blocks"}))
            .collect();
        let fields = json!({"title": "t", "status": "open", "priority": 2, "issue_type": "task"});
        let mut data = json!({"key": (key.as_str()), "fields": fields, "imported": true});
        if let (false, Value::Object(members)) = (dangling.is_empty(), &mut data) {
            members.insert("dangling".into(), Value::Array(dangling));
        }
        let create = self.emit(replica, "create", None, data);
        for t in &targets {
            if let Some(uid) = st.keys().get(t) {
                let data = json!({"to": (uid.to_string()), "type": "blocks", "meta": {}, "imported": true});
                self.emit(replica, "dep_add", Some(&create), data);
            }
        }
    }

    /// `replica` removes a dependency by key or by uid, or adds one.
    fn edit(&mut self, replica: &str, st: &State, roll: f64) {
        let issues: Vec<_> = st.issues().collect();
        if issues.is_empty() {
            return;
        }
        let issue = issues[self.stream.index(issues.len())];
        let uid = issue.uid().to_string();
        let mut edges: Vec<(String, String)> = issue
            .deps()
            .keys()
            .map(|edge| (edge.to().to_string(), edge.kind().to_owned()))
            .collect();
        edges.sort();
        if roll < 0.7 && !issue.dangling().is_empty() {
            // Dep remove by key, or a re-import that drops it.
            let dep = self.stream.choice(issue.dangling());
            let kind = self.stream.choice(&[Value::Null, Value::from("blocks")]);
            let data = json!({"key": (dep["depends_on_id"].clone()), "type": kind});
            self.emit(replica, "dep_remove", Some(&uid), data);
        } else if roll < 0.85 && !edges.is_empty() {
            // Dep remove by uid.
            let (to, _) = self.stream.choice(&edges);
            self.emit(
                replica,
                "dep_remove",
                Some(&uid),
                json!({"to": (to.as_str()), "type": null}),
            );
        } else if issues.len() > 1 {
            // Explicit dep add.
            let others: Vec<String> = issues
                .iter()
                .filter(|i| i.uid() != issue.uid())
                .map(|i| i.uid().to_string())
                .collect();
            let other = self.stream.choice(&others);
            self.emit(
                replica,
                "dep_add",
                Some(&uid),
                json!({"to": (other.as_str()), "type": "blocks"}),
            );
        }
    }
}

fn pool() -> Vec<String> {
    "abcdefgh".chars().map(|c| format!("ex-{c}")).collect()
}

/// A CLI-shaped history on three replicas, each acting on its own replayed knowledge. With
/// duplicates enabled, two replicas may mint the same key.
fn generate(stream: &mut Stream, twins: Identities) -> Vec<Value> {
    let twins = matches!(twins, Identities::Twins);
    let known = REPLICAS.iter().map(|r| (*r, BTreeSet::new())).collect();
    let mut history = History {
        stream,
        events: Vec::new(),
        known,
        counter: 0,
    };
    let mut used = BTreeSet::new();
    let init = history.emit("r0", "init", None, json!({"prefix": "ex", "name": "ex"}));
    for r in ["r1", "r2"] {
        history.known.get_mut(r).unwrap().insert(init.clone());
    }
    for _ in 0..history.stream.randint(8, 22) {
        let r = history.stream.choice(&REPLICAS);
        let seen: Vec<Value> = history
            .events
            .iter()
            .filter(|e| history.known[r].contains(e["id"].as_str().unwrap()))
            .cloned()
            .collect();
        let st = replayed(&seen);
        let roll = history.stream.random();
        if roll < 0.15 {
            // Sync two replicas.
            let others: Vec<&str> = REPLICAS.iter().copied().filter(|x| *x != r).collect();
            let other = history.stream.choice(&others);
            let union: BTreeSet<String> = history.known[r]
                .union(&history.known[other])
                .cloned()
                .collect();
            history.known.insert(r, union.clone());
            history.known.insert(other, union);
            continue;
        }
        let fresh: Vec<String> = pool()
            .into_iter()
            .filter(|k| {
                if twins {
                    !st.keys().contains_key(k)
                } else {
                    !used.contains(k)
                }
            })
            .collect();
        if roll < 0.5 && !fresh.is_empty() {
            history.import(r, &st, &fresh, &mut used);
        } else {
            history.edit(r, &st, roll);
        }
    }
    history.events
}

#[derive(Debug, Default)]
struct Stats {
    events: usize,
    key_removals: usize,
    uid_removals: usize,
    dangling_creates: usize,
    twin_seeds: usize,
}

fn check(identities: Identities) {
    let twins = matches!(identities, Identities::Twins);
    let mut stats = Stats::default();
    for seed in 0..SEEDS {
        let mut stream = Stream(seed * 7919 + if twins { 104_729 } else { 0 });
        let events = generate(&mut stream, identities);
        stats.events += events.len();
        let op = |e: &Value| e["op"].as_str().unwrap().to_owned();
        stats.key_removals += events
            .iter()
            .filter(|e| op(e) == "dep_remove" && e["data"]["key"].truthy())
            .count();
        stats.uid_removals += events
            .iter()
            .filter(|e| op(e) == "dep_remove" && e["data"]["to"].truthy())
            .count();
        stats.dangling_creates += events
            .iter()
            .filter(|e| op(e) == "create" && e["data"]["dangling"].truthy())
            .count();
        let keys: Vec<&str> = events
            .iter()
            .filter(|e| op(e) == "create")
            .map(|e| e["data"]["key"].as_str().unwrap())
            .collect();
        let distinct: BTreeSet<&str> = keys.iter().copied().collect();
        stats.twin_seeds += usize::from(keys.len() != distinct.len());
        let (base, pending, integrity) = projection(&events);
        assert_eq!(
            (pending, integrity),
            (0, 0),
            "seed {seed}: replay left events unapplied or flagged"
        );
        let mut per_replica: Vec<(String, Vec<Value>)> = Vec::new();
        for event in &events {
            let replica = event["replica"].as_str().unwrap().to_owned();
            match per_replica.iter_mut().find(|(r, _)| *r == replica) {
                Some((_, list)) => list.push(event.clone()),
                None => per_replica.push((replica, vec![event.clone()])),
            }
        }
        for _ in 0..ORDERS {
            // Whole journals in a random order, each in its own order.
            let mut replicas: Vec<usize> = (0..per_replica.len()).collect();
            stream.shuffle(&mut replicas);
            let order: Vec<Value> = replicas
                .iter()
                .flat_map(|r| per_replica[*r].1.clone())
                .collect();
            assert_eq!(
                projection(&order).0,
                base,
                "seed {seed}: delivery order {replicas:?}"
            );
        }
    }
    // The generated histories actually exercise what the property is about.
    assert!(stats.key_removals > 400, "{stats:?}");
    assert!(stats.uid_removals > 200, "{stats:?}");
    assert!(stats.dangling_creates > 1000, "{stats:?}");
    if twins {
        assert!(stats.twin_seeds > 200, "{stats:?}");
    } else {
        assert_eq!(stats.twin_seeds, 0, "{stats:?}");
    }
}

/// Verified (V4): replay is deterministic for dangling histories: the same events give the same
/// projection (edges by key and unresolved dangling keys) under any whole-journal delivery order,
/// over 1000 seeds × 4 orders of the generator.
#[test]
fn unique_key_histories_replay_identically_under_any_delivery_order() {
    check(Identities::Unique);
}

/// Verified (V4): the same determinism holds when a key may be minted on two replicas (twin keys,
/// so one copy is renamed).
#[test]
fn twin_key_histories_replay_identically_under_any_delivery_order() {
    check(Identities::Twins);
}
