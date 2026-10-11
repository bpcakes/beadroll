//! The ancestry index answers exactly what the bitsets it replaced answered (Python's `seen` and
//! `own_anc`), on histories shaped to defeat it: attempts that build on nothing, on other machines'
//! attempts or on any older event, so that chains cannot follow machines; copied identities that
//! break a journal's chain; quarantined ids; causes the journal does not hold.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::id::EventId;
use crate::journal::Journal;
use crate::json::{Object, Value};
use crate::order::{causal_order, effective_deps};
use crate::replay::replay;

/// A seeded generator (SplitMix64): the same draws on every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
}

const REPLICAS: [&str; 4] = ["0000aa", "0000bb", "0000cc", "0000dd"];

fn id(n: usize, millis: u64) -> String {
    format!(
        "{:08x}-{:04x}-7000-8000-{n:012x}",
        millis >> 16,
        millis & 0xffff
    )
}

fn event(id: &str, replica: &str, deps: &[String], text: &str) -> Value {
    let mut data = Object::new();
    data.insert("text".into(), Value::from(text));
    let mut event = Object::new();
    event.insert("v".into(), Value::from(1));
    event.insert("id".into(), Value::from(id));
    event.insert("replica".into(), Value::from(replica));
    let deps = deps.iter().map(|d| Value::from(d.as_str())).collect();
    event.insert("deps".into(), Value::Array(deps));
    event.insert("op".into(), Value::from("comment"));
    event.insert("issue".into(), Value::from(id));
    event.insert("data".into(), Value::from(data));
    Value::from(event)
}

/// A hostile history of `count` events on four machines, about half of them in attempt files; and
/// ids nobody wrote, which some events name.
fn history(rng: &mut Rng, count: usize) -> (Journal, Vec<EventId>) {
    let mut ids: Vec<String> = Vec::new();
    let mut last: Vec<Option<usize>> = vec![None; REPLICAS.len()];
    let mut segments: Vec<Vec<Value>> = vec![Vec::new(); REPLICAS.len()];
    let mut attempts: Vec<(usize, Value)> = Vec::new();
    let mut missing = Vec::new();
    for n in 0..count {
        let replica = rng.below(REPLICAS.len());
        let attempt = rng.chance(50);
        let mut deps = Vec::new();
        // Usually the writer's own latest event; an attempt often builds on nothing of its own.
        if let Some(previous) = last.get(replica).copied().flatten()
            && rng.chance(if attempt { 40 } else { 95 })
        {
            deps.push(ids[previous].clone());
        }
        for _ in 0..rng.below(3) {
            if !ids.is_empty() {
                deps.push(ids[rng.below(ids.len())].clone());
            }
        }
        if rng.chance(2) {
            let nobody = id(count + n, 1_700_000_000_000 + rng.below(50_000) as u64);
            missing.push(EventId::parse(&nobody).unwrap());
            deps.push(nobody);
        }
        let name = id(n, 1_700_000_000_000 + rng.below(50_000) as u64);
        let value = event(&name, REPLICAS[replica], &deps, "x");
        if attempt {
            attempts.push((replica, value));
        } else {
            segments[replica].push(value);
        }
        if rng.chance(2) {
            // The same id with other contents: quarantined.
            attempts.push((replica, event(&name, REPLICAS[replica], &deps, "y")));
        }
        ids.push(name);
        last[replica] = Some(n);
    }
    let mut journal = Journal::new();
    for (replica, events) in segments.into_iter().enumerate() {
        for value in events {
            journal.add(value, REPLICAS[replica], "segment", true);
        }
    }
    for (replica, value) in attempts {
        journal.add(value, REPLICAS[replica], "attempt", false);
    }
    journal.settle();
    (journal, missing)
}

/// `saw` as the bitsets answered it: every event's furthest position in each machine's journal, and
/// the set of unsequenced events in its past, built in causal order as the old replay did.
fn bitset_saw(journal: &Journal) -> impl Fn(EventId, EventId) -> bool + '_ {
    let (ordered, _) = causal_order(journal);
    let mut seq: HashMap<EventId, (usize, usize)> = HashMap::new();
    let mut applicable: Vec<Vec<usize>> = Vec::new();
    for (replica, ids) in journal.order().values().enumerate() {
        let placeholder = |id: &EventId| {
            journal
                .event(*id)
                .is_none_or(|e| matches!(e.op_name(), "quarantined" | "invalid"))
        };
        applicable.push(
            ids.iter()
                .enumerate()
                .filter(|(_, id)| !placeholder(id))
                .map(|(n, _)| n)
                .collect(),
        );
        for (number, id) in ids.iter().enumerate() {
            seq.insert(*id, (replica, number));
        }
    }
    let mut seen: HashMap<EventId, BTreeMap<usize, i64>> = HashMap::new();
    let mut anc: HashMap<EventId, BTreeSet<EventId>> = HashMap::new();
    for id in ordered {
        let event = journal.event(id).unwrap();
        let (mut sees, mut past) = (BTreeMap::new(), BTreeSet::new());
        for dep in effective_deps(journal, event) {
            let Some(theirs) = seen.get(&dep) else {
                continue;
            };
            for (replica, number) in theirs {
                let mine = sees.entry(*replica).or_insert(*number);
                *mine = (*mine).max(*number);
            }
            past.extend(anc[&dep].iter().copied());
            if !seq.contains_key(&dep) {
                past.insert(dep);
            }
        }
        anc.insert(id, past);
        if let Some(&(replica, number)) = seq.get(&id) {
            let positions = &applicable[replica];
            let at = positions.partition_point(|p| *p < number);
            let predecessor = at
                .checked_sub(1)
                .map_or(-1, |before| positions[before] as i64);
            if sees.get(&replica).copied().unwrap_or(-1) < predecessor {
                seen.insert(id, sees);
                continue;
            }
            let mine = sees.entry(replica).or_insert(-1);
            *mine = (*mine).max(number as i64);
        }
        seen.insert(id, sees);
    }
    move |event, other| {
        if journal
            .event(event)
            .is_some_and(|e| e.deps().contains(&other))
        {
            return true;
        }
        if let Some(&(replica, number)) = seq.get(&other) {
            let sees = seen.get(&event).and_then(|s| s.get(&replica)).copied();
            return sees.unwrap_or(-1) >= number as i64;
        }
        anc.get(&event).is_some_and(|past| past.contains(&other))
    }
}

#[test]
fn every_answer_is_the_bitsets_answer_on_hostile_histories() {
    let mut rng = Rng(20_261_010);
    let mut wider = 0;
    for _ in 0..24 {
        let count = 50 + rng.below(250);
        let (journal, missing) = history(&mut rng, count);
        let mut settled = journal.clone();
        settled.settle();
        let st = replay(journal).unwrap();
        let expected = bitset_saw(&settled);
        let ids: Vec<EventId> = st.events().chain(missing).collect();
        for &a in &ids {
            for &b in &ids {
                assert_eq!(st.saw(a, b), expected(a, b), "{a} saw {b}");
            }
        }
        let chains = st.knowledge.ancestry.lengths.len();
        if chains > REPLICAS.len() {
            wider += 1;
        }
    }
    // Most histories need more chains than machines: the index is not leaning on their shape.
    assert!(
        wider > 12,
        "{wider} of 24 histories needed more chains than machines"
    );
}

#[test]
fn a_valid_history_needs_a_chain_per_machine_that_writes_attempts() {
    // Every event builds on everything its writer holds, as `Tx.emit` writes them.
    let mut journal = Journal::new();
    let mut heads: Vec<String> = Vec::new();
    for n in 0..600 {
        let replica = REPLICAS[n % REPLICAS.len()];
        let name = id(n, 1_700_000_000_000 + n as u64);
        let value = event(&name, replica, &heads, "x");
        journal.add(value, replica, "f", n % 3 != 0);
        heads = vec![name];
    }
    let st = replay(journal).unwrap();
    assert!(st.integrity().is_empty());
    assert!(st.knowledge.ancestry.lengths.len() <= REPLICAS.len());
    let first = EventId::parse(&id(1, 1_700_000_000_001)).unwrap();
    let last = EventId::parse(&id(599, 1_700_000_000_599)).unwrap();
    assert!(st.saw(last, first));
    assert!(!st.saw(first, last));
}
