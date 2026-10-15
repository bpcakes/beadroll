//! Machine 0's store of a world taking in what the machines write, batch by batch, its projection
//! extended by each or declining it, every extension compared with projecting the whole journal.

use std::collections::BTreeMap;

use crate::id::EventId;
use crate::json::{Object, Value};
use crate::replay::replay;

use super::super::{Extended, project};
use super::{TextFile, World, equal, flawed, load, read_back, read_in, records, same, whole};

/// What [`differential`] saw: the batches taken in, the merges among them, the batches that came to
/// a projection that could not be extended, and the local writes declined by one that could that are
/// not takeovers (by seed); with the extensions applied to a projection that held each kind of
/// integrity entry (by [`flawed::kinds`]), and the entries of each kind the batches brought.
#[derive(Default, Debug)]
pub(super) struct Tally {
    pub(super) applied: usize,
    pub(super) merged: usize,
    pub(super) closed: usize,
    pub(super) declined: Vec<u64>,
    pub(super) past: BTreeMap<&'static str, usize>,
    pub(super) brought: BTreeMap<&'static str, usize>,
}

/// Machine 0's store, of each seed's world, takes in 60 batches, each extending its projection or
/// declined; every extension must be what projecting the whole journal makes. A world `flawed` has
/// machines that also write what replay reports, and one whose identity was copied.
pub(super) fn differential(seeds: impl IntoIterator<Item = u64>, flawed: bool) -> Tally {
    let mut tally = Tally::default();
    for seed in seeds {
        let mut world = if flawed {
            World::flawed(20_261_012 + seed)
        } else {
            World::new(20_261_012 + seed)
        };
        let mut data = Object::new();
        data.insert("prefix".into(), Value::from("pr"));
        world.write(0, "init", None, data);
        let mut held: Vec<usize> = vec![0];
        let mut projection = project(load(&world, &held)).unwrap();
        let mut file = TextFile::new();
        for _ in 0..60 {
            let batch = world.next_batch();
            if world.rng.chance(50) {
                projection = read_back(&projection, &mut file);
            }
            held.extend(&batch);
            let full = project(load(&world, &held)).unwrap();
            assert!(same(full.state(), &replay(load(&world, &held)).unwrap()));
            let local = batch.iter().all(|at| world.events[*at].machine == 0);
            let before = flawed::kinds(projection.state());
            let open = projection.extendable();
            tally.closed += usize::from(!open);
            let records = records(&world, &batch);
            // as a store does: the texts of the issues the batch is about read in first
            let about = (records.iter())
                .filter_map(|record| record.value.get("issue").and_then(Value::as_str))
                .filter_map(EventId::parse)
                .collect();
            read_in(projection.state_mut(), &file, Some(about));
            for (kind, n) in flawed::kinds(full.state()) {
                let new = n.saturating_sub(before.get(kind).copied().unwrap_or(0));
                *tally.brought.entry(kind).or_default() += new;
            }
            projection = match projection.extend(records).unwrap() {
                Extended::Applied(extended) => {
                    assert!(equal(&whole(&extended, &file), &full), "seed {seed}");
                    for kind in before.into_keys() {
                        *tally.past.entry(kind).or_default() += 1;
                    }
                    tally.applied += 1;
                    tally.merged += usize::from(!local);
                    *extended
                }
                Extended::Replay => {
                    if local && open && !world.takes_over(&batch) {
                        tally.declined.push(seed);
                    }
                    full
                }
            };
        }
    }
    tally
}
