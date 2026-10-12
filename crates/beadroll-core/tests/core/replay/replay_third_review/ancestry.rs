//! `ReplayThirdReview`, the ancestry of a long mixed history: events in journals and in attempt
//! files, on six machines.

use std::collections::BTreeSet;

use beadroll_core::replay::replay;
use beadroll_test_support::{Json, json};

use super::{Fixture, id_of, journal, uid};

/// A deterministic SplitMix64 stream for ancestry property checks.
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
        usize::try_from(self.next() % u64::try_from(n).unwrap()).unwrap()
    }

    /// True with probability `tenths` in ten.
    fn chance(&mut self, tenths: u64) -> bool {
        self.next() % 10 < tenths
    }
}

/// 1,500 comments on six machines, every third in an attempt file (unsequenced, as a conditional edit
/// made with a claim is): `saw` answers exactly the graph's ancestry, and the export does not depend
/// on the order the machines' journals are read in.
#[test]
fn mixed_detached_ownership_ancestry_matches_exact_graph() {
    let mut rng = Rng(71029);
    let mut f = Fixture::new();
    let (init, create) = f.base();
    let mut events = vec![init, create.clone()];
    let mut ancestors: Vec<BTreeSet<usize>> = vec![BTreeSet::new(), BTreeSet::from([0])];
    let mut heads: Vec<(char, usize)> = vec![('a', 1)];
    let mut detached = BTreeSet::new();
    for n in 0..1500 {
        let machine = "abcdef".chars().nth(rng.below(6)).unwrap();
        let own = heads
            .iter()
            .find(|(m, _)| *m == machine)
            .map_or(1, |(_, h)| *h);
        let mut deps = vec![own];
        for (m, head) in &heads {
            if *m != machine && rng.chance(1) {
                deps.push(*head);
            }
        }
        let names: Vec<String> = deps.iter().map(|d| id_of(&events[*d]).to_owned()).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let text = n.to_string();
        let event = f.event(
            "comment",
            &machine.to_string(),
            &names,
            Some(id_of(&create)),
            json!({"text": (text.as_str())}),
        );
        if n % 3 == 0 {
            detached.insert(id_of(&event).to_owned()); // a conditional edit shares ownership-file placement
        }
        let mut past: BTreeSet<usize> = deps.iter().copied().collect();
        for d in &deps {
            past.extend(ancestors[*d].iter().copied());
        }
        ancestors.push(past);
        events.push(event);
        match heads.iter_mut().find(|(m, _)| *m == machine) {
            Some(head) => head.1 = events.len() - 1,
            None => heads.push((machine, events.len() - 1)),
        }
    }
    let state = replay(journal(&events, &detached)).unwrap();
    assert!(state.integrity().is_empty());
    assert!(state.pending().is_empty());
    for _ in 0..10_000 {
        let a = rng.below(events.len());
        let b = (a + 1 + rng.below(events.len() - 1)) % events.len();
        assert_eq!(
            state.saw(uid(&events[a]), uid(&events[b])),
            ancestors[a].contains(&b),
            "{a} saw {b}"
        );
    }
    let expected = state.export_records();
    for _ in 0..12 {
        let mut order: Vec<char> = "abcdef".chars().collect();
        for at in (1..order.len()).rev() {
            order.swap(at, rng.below(at + 1));
        }
        let variant: Vec<Json> = order
            .iter()
            .flat_map(|m| {
                let name = m.to_string();
                events
                    .iter()
                    .filter(move |e| e["replica"].as_str() == Some(name.as_str()))
                    .cloned()
            })
            .collect();
        assert_eq!(
            replay(journal(&variant, &detached))
                .unwrap()
                .export_records(),
            expected
        );
    }
}
