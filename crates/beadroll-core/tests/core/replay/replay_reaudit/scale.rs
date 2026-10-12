//! Replay at scale: thousands of seeded events across six machines, delivered in different
//! orders. Assertions hold for every generated history.

use std::collections::BTreeMap;

use beadroll_core::event::{Field, REGISTERS};
use beadroll_core::id::EventId;
use beadroll_core::journal::Journal;
use beadroll_core::query::Edges;
use beadroll_core::replay::replay;
use beadroll_test_support::Json;
use beadroll_test_support::events::journal;
use beadroll_test_support::json;

use super::{Fixture, id, projection};

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

    /// A number below `n`.
    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % n as u64).unwrap()
    }

    /// True with probability `p`.
    fn chance(&mut self, p: f64) -> bool {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64 <= p
    }

    /// One of `items`.
    fn choice<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }

    /// `count` distinct orders of the six machines, as `rng.sample(permutations("abcdef"), count)`.
    fn orders(&mut self, count: usize) -> Vec<Vec<&'static str>> {
        let mut all: Vec<Vec<&'static str>> = vec![Vec::new()];
        for _ in 0..MACHINES.len() {
            let mut longer = Vec::new();
            for order in &all {
                for machine in MACHINES.iter().filter(|m| !order.contains(m)) {
                    let mut next = order.clone();
                    next.push(machine);
                    longer.push(next);
                }
            }
            all = longer;
        }
        for at in 0..count {
            let pick = at + self.below(all.len() - at);
            all.swap(at, pick);
        }
        all.truncate(count);
        all
    }
}

const MACHINES: [&str; 6] = ["a", "b", "c", "d", "e", "f"];

/// The events of `rows`, machine by machine in `order`, each machine's in its own order.
fn build(rows: &[(Json, &str)], order: &[&str]) -> Journal {
    let mut j = Journal::new();
    for machine in order {
        for (event, owner) in rows.iter().filter(|(_, owner)| owner == machine) {
            j.add(event.clone(), owner, "", true);
        }
    }
    j
}

/// The 3,600 events of the mixed-envelope test: on every machine, five of each six invalid one way
/// or another, the sixth a valid comment. Also the number of valid comments.
fn mixed_envelope_rows(f: &mut Fixture, rng: &mut Rng) -> (Vec<(Json, &'static str)>, usize) {
    let (init, create) = f.fixture();
    let uid = id(&create);
    let mut rows = vec![(init, "a"), (create, "a")];
    let mut heads: BTreeMap<&str, String> = BTreeMap::from([("a", uid.clone())]);
    let mut valid_comments = 0;
    for n in 0..3600 {
        let machine = *rng.choice(&MACHINES);
        let mut deps = vec![heads.get(machine).cloned().unwrap_or_else(|| uid.clone())];
        if rng.chance(0.2) {
            deps.extend(
                heads
                    .iter()
                    .filter(|(m, _)| **m != machine)
                    .map(|(_, head)| head.clone()),
            );
        }
        let deps: Vec<&str> = deps.iter().map(String::as_str).collect();
        let text = json!({"text": (format!("comment {n}"))});
        let event = match n % 6 {
            0 => f.event(
                "comment",
                machine,
                &deps,
                Some(&uid),
                json!(["invalid type"]),
            ),
            1 => f.event("unknown_operation", machine, &deps, Some(&uid), text),
            2 => with(
                f.event("comment", machine, &deps, Some(&uid), text),
                "v",
                json!(2),
            ),
            3 => with(
                f.event("comment", machine, &deps, Some(&uid), text),
                "actor",
                json!(["invalid type"]),
            ),
            4 => f.event("set", machine, &deps, Some(&uid), json!({"fields": []})),
            _ => {
                valid_comments += 1;
                f.event("comment", machine, &deps, Some(&uid), text)
            }
        };
        heads.insert(machine, id(&event));
        rows.push((event, machine));
    }
    (rows, valid_comments)
}

fn with(mut event: Json, name: &str, value: Json) -> Json {
    if let Json::Object(members) = &mut event {
        members.insert(name.into(), value);
    }
    event
}

#[test]
fn six_machine_3600_mixed_envelope_events_are_scoped_and_deterministic() {
    let mut rng = Rng(901_834);
    let mut f = Fixture::new();
    let (rows, valid_comments) = mixed_envelope_rows(&mut f, &mut rng);
    let uid = id(&rows[1].0);
    let base = build(&rows, &MACHINES);
    let st = replay(base.clone()).unwrap();
    assert!(st.pending().is_empty());
    assert!(st.chain_breaks().is_empty());
    assert_eq!(st.integrity().len(), 3000);
    assert_eq!(
        st.issue_named(&uid).unwrap().comments().len(),
        valid_comments
    );
    let expected = projection(base);
    for order in rng.orders(18) {
        assert_eq!(projection(build(&rows, &order)), expected, "{order:?}");
    }
}

#[test]
fn deep_valid_dependency_graph_does_not_overflow_cycle_scan() {
    let mut f = Fixture::new();
    let (init, first) = f.fixture();
    let mut last = id(&first);
    let mut issues = vec![last.clone()];
    let mut events = vec![init, first];
    for n in 0..1500 {
        let data =
            json!({"key": (format!("ex-chain-{n}")), "fields": {"title": (format!("chain {n}"))}});
        let event = f.event("create", "a", &[&last], None, data);
        last = id(&event);
        issues.push(last.clone());
        events.push(event);
    }
    for pair in issues.windows(2) {
        let event = f.event(
            "dep_add",
            "a",
            &[&last],
            Some(&pair[0]),
            json!({"to": (pair[1].as_str()), "type": "blocks"}),
        );
        last = id(&event);
        events.push(event);
    }
    let st = replay(journal(&events)).unwrap();
    assert!(st.integrity().is_empty());
    assert!(st.cycles(Edges::WaitsFor).is_empty());
}

/// The payload of event `n` of the convergence test: a register written, a label added or removed,
/// or a comment.
fn valid_payload(rng: &mut Rng, n: usize) -> (&'static str, Json) {
    let op = *rng.choice(&["set", "label_add", "label_remove", "comment"]);
    let data = match op {
        "set" => {
            let field = *rng.choice(&REGISTERS);
            let value = match field {
                Field::Priority => json!(i64::try_from(n % 5).unwrap()),
                Field::EstimatedMinutes => json!(i64::try_from(n % 180).unwrap()),
                Field::IssueType => json!("task"),
                Field::Title => json!(format!("value{}", n % 31)),
                _ => rng
                    .choice(&[Json::Null, json!(format!("value{}", n % 31))])
                    .clone(),
            };
            let fields = [(field.name().to_owned(), value)].into_iter().collect();
            json!({"fields": (Json::Object(Box::new(fields)))})
        }
        "label_add" | "label_remove" => json!({"labels": [(format!("label{}", n % 13))]}),
        _ => json!({"text": (format!("comment{n}"))}),
    };
    (op, data)
}

/// Each event's ancestors, as a set of positions in the list of events.
#[derive(Clone, Default)]
struct Bits(Vec<u64>);

impl Bits {
    fn insert(&mut self, at: usize) {
        if self.0.len() <= at / 64 {
            self.0.resize(at / 64 + 1, 0);
        }
        self.0[at / 64] |= 1 << (at % 64);
    }

    fn union(&mut self, other: &Bits) {
        if self.0.len() < other.0.len() {
            self.0.resize(other.0.len(), 0);
        }
        for (mine, theirs) in self.0.iter_mut().zip(&other.0) {
            *mine |= theirs;
        }
    }

    fn contains(&self, at: usize) -> bool {
        self.0
            .get(at / 64)
            .is_some_and(|word| word >> (at % 64) & 1 == 1)
    }
}

/// The 6,000 valid events of the convergence test, and the exact ancestors of each.
fn converging_events(f: &mut Fixture, rng: &mut Rng) -> (Vec<Json>, Vec<Bits>) {
    let (init, create) = f.fixture();
    let uid = id(&create);
    let mut ancestry = vec![Bits::default(), Bits::default()];
    ancestry[1].insert(0);
    let mut index: BTreeMap<String, usize> = BTreeMap::from([(id(&init), 0), (uid.clone(), 1)]);
    let mut events = vec![init, create];
    let mut heads: Vec<(&str, String)> = vec![("a", uid.clone())];
    for n in 0..5998 {
        let machine = *rng.choice(&MACHINES);
        let own = heads
            .iter()
            .find(|(m, _)| *m == machine)
            .map(|(_, head)| head.clone());
        let mut deps = vec![own.unwrap_or_else(|| uid.clone())];
        for (m, head) in &heads {
            if *m != machine && rng.chance(0.08) {
                deps.push(head.clone());
            }
        }
        let (op, data) = valid_payload(rng, n);
        let refs: Vec<&str> = deps.iter().map(String::as_str).collect();
        let event = f.event(op, machine, &refs, Some(&uid), data);
        let mut ancestors = Bits::default();
        for dep in &deps {
            let at = index[dep];
            ancestors.union(&ancestry[at]);
            ancestors.insert(at);
        }
        index.insert(id(&event), events.len());
        ancestry.push(ancestors);
        match heads.iter_mut().find(|(m, _)| *m == machine) {
            Some((_, head)) => *head = id(&event),
            None => heads.push((machine, id(&event))),
        }
        events.push(event);
    }
    (events, ancestry)
}

#[test]
fn valid_six_machine_6000_events_converge_and_ancestry_is_exact() {
    let mut rng = Rng(901_833);
    let mut f = Fixture::new();
    let (events, ancestry) = converging_events(&mut f, &mut rng);
    let base = journal(&events);
    let expected = projection(base.clone());
    let st = replay(base.clone()).unwrap();
    assert!(st.integrity().is_empty());
    assert!(st.pending().is_empty());
    let uuid = |at: usize| EventId::parse(&id(&events[at])).unwrap();
    for _ in 0..10_000 {
        let later = 1 + rng.below(events.len() - 1);
        let earlier = rng.below(later);
        assert_eq!(
            st.saw(uuid(later), uuid(earlier)),
            ancestry[later].contains(earlier),
            "{later} saw {earlier}"
        );
    }
    // Arbitrary interleaving of complete machine journals, preserving each journal's bytes.
    let written: std::collections::HashMap<EventId, &Json> = (events.iter())
        .map(|event| (EventId::parse(&id(event)).unwrap(), event))
        .collect();
    for order in rng.orders(36) {
        let mut j = Journal::new();
        for machine in &order {
            for eid in base
                .order()
                .get(*machine)
                .map(Vec::as_slice)
                .unwrap_or_default()
            {
                let event = written[eid].clone();
                j.add(event.clone(), machine, "", true);
                j.add(event, machine, "", true);
            }
        }
        assert_eq!(projection(j), expected, "{order:?}");
    }
}
