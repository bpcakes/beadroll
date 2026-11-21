//! Histories whose journals hold what keeps the frontier off a machine's last record (AT102): a
//! cycle of causes, a cause no journal holds yet (later written, by another machine), an operation
//! replay does not know, a placeholder (an operation that is no text), a record written with the
//! clock turned back, in the store's own journal as in others. Machine 0 builds each write on what
//! its projection names (`Projection::causes_for`), as a transaction builds it, and replay must
//! take it into its journal's chain, or not, exactly as those causes said it would; and every
//! extension is still what projecting the whole journal makes, or declines.

use super::taken::differential;
use super::{MACHINES, Machine, World, load};
use crate::id::EventId;
use crate::json::{Object, Value};
use crate::projection::tests::HistoryKind;
use crate::replay::replay;
use crate::state::{Place, State};

impl World {
    /// The world of `seed` of `kind`.
    pub(super) fn of(seed: u64, kind: HistoryKind) -> World {
        match kind {
            HistoryKind::Valid => World::new(seed),
            HistoryKind::Flawed => World::flawed(seed),
            HistoryKind::Chained => World::chained(seed),
        }
    }

    /// A flawed world ([`World::flawed`]) whose machines, machine 0 among them, also write what
    /// keeps the frontier off a journal's last record ([`World::chain_flaw`]), and whose machine 0
    /// builds on what its projection names. What never lets go is in some worlds only, as it
    /// leaves the rest of a history to replay: in a third, cycles of causes, which leave the
    /// projection waiting; in a fourth, a fifth machine, a copy of machine 0's identity, that now
    /// and then breaks its journal's chain, setting aside every later write of the store.
    fn chained(seed: u64) -> World {
        let mut world = World::flawed(seed);
        if seed % 4 == 1 {
            world.machines.push(Machine::default());
            world.folders.push(MACHINES[0]);
        }
        world.chained = true;
        world.cycles = seed.is_multiple_of(3);
        world
    }

    /// One thing machine `m` writes that keeps the frontier off a journal's last record, or that
    /// releases what waited (an event of an id one names, by a machine other than the store under
    /// test): two comments each naming the other, a comment naming an id no event has, an operation
    /// replay does not know, a placeholder, or what machine `m` does with its clock turned back. Of
    /// machine 0's, only the last are writes of a transaction, and judged.
    pub(super) fn chain_flaw(&mut self, m: usize) {
        let (issues, _) = self.issues(m);
        let Some(issue) = self.rng.pick(&issues).cloned() else {
            return self.act(m);
        };
        let judged = self.built.len();
        let text = |text: &str| {
            let mut data = Object::new();
            data.insert("text".into(), Value::from(text));
            data
        };
        // a cycle in the store's own journal would leave every later write of it waiting
        match self.rng.below(5) {
            0 if m != 0 && self.cycles => {
                let second = self.mint();
                self.write(m, "comment", Some(issue.clone()), text("one"));
                self.add_cause(self.events.len() - 1, &second);
                self.next_id = Some(second);
                self.write(m, "comment", Some(issue), text("two"));
            }
            0 | 1 => {
                let owed = self.mint();
                self.write(m, "comment", Some(issue), text("waits"));
                self.add_cause(self.events.len() - 1, &owed);
                self.owed.push(owed);
            }
            2 => drop(self.write(m, "future_op", Some(issue), Object::new())),
            3 => {
                // a placeholder: an op that is no text, no applicable position of its journal
                self.write(m, "comment", Some(issue), text("placeholder"));
                let at = self.events.len() - 1;
                let mut event = self.events[at]
                    .value
                    .as_object()
                    .cloned()
                    .unwrap_or_default();
                event.insert("op".into(), Value::from(5));
                self.events[at].value = Value::from(event);
            }
            _ => {
                let now = self.clock;
                self.clock = now.saturating_sub(1 + self.rng.below(40) as u64);
                self.next_id = Some(self.mint());
                self.clock = now;
                // what it does is judged, as any write of a transaction is
                return self.act(m);
            }
        }
        self.built.truncate(judged);
    }

    /// Machine `m`, not the store under test, before it takes in the latest, writes more often
    /// than not an event of an id an event names that no event has, releasing what waits for it:
    /// one it knows no event naming, so as not to build on what waits for it; whether it did.
    pub(super) fn pays(&mut self, m: usize) -> bool {
        if !self.chained || m == 0 || !self.rng.chance(80) {
            return false;
        }
        let names = |world: &World, owed: &str| {
            (world.machines[m].known.iter()).any(|at| match world.events[*at].value.get("deps") {
                Some(Value::Array(deps)) => deps.iter().any(|dep| dep.as_str() == Some(owed)),
                _ => false,
            })
        };
        let payable: Vec<usize> = (0..self.owed.len())
            .filter(|at| !names(self, &self.owed[*at]))
            .collect();
        let (issues, _) = self.issues(m);
        let (Some(&at), Some(issue)) = (self.rng.pick(&payable), self.rng.pick(&issues).cloned())
        else {
            return false;
        };
        self.next_id = Some(self.owed.swap_remove(at));
        let mut data = Object::new();
        data.insert("text".into(), Value::from("owed"));
        self.write(m, "comment", Some(issue), data);
        true
    }

    /// The event written at `at`, naming `cause` too.
    fn add_cause(&mut self, at: usize, cause: &str) {
        let mut event = self.events[at]
            .value
            .as_object()
            .cloned()
            .unwrap_or_default();
        let mut deps = match event.get("deps") {
            Some(Value::Array(deps)) => deps.clone(),
            _ => Vec::new(),
        };
        deps.push(Value::from(cause));
        event.insert("deps".into(), Value::Array(deps));
        self.events[at].value = Value::from(event);
    }
}

/// Whether replay took the event `id` in `state` into its journal's chain: observed, its causes
/// before it, and its position not one replay recorded as a break in that chain (what a write
/// built as a transaction builds it must be; that its operation then took effect is another
/// question, which the differential tests ask of the state).
fn in_chain(state: &State, id: &str) -> bool {
    let knowledge = &state.knowledge;
    let Some(at) = EventId::parse(id).and_then(|id| knowledge.events.get_index_of(&id)) else {
        return false;
    };
    let observed = knowledge.ancestry.observed(at);
    let broke = match knowledge.ancestry.place(at) {
        Place::Sequenced(chain, number) => (knowledge.replicas.get(chain as usize))
            .and_then(|machine| knowledge.chain_breaks.get(machine))
            .is_some_and(|breaks| breaks.contains(&(number as usize))),
        _ => false,
    };
    observed && !broke
}

/// What [`judged`] saw: machine 0's writes judged; those built on its last record beside the
/// frontier, that record's id sorting before another of its journal's (written with the clock
/// turned back), on a frontier that waited, on a last record that waited, after a broken chain,
/// and after a placeholder last in its journal; and those replay took into its chain.
#[derive(Debug, Default)]
struct Judged {
    writes: usize,
    predecessors: usize,
    turned_back: usize,
    waits: usize,
    predecessor_waits: usize,
    broken: usize,
    after_placeholder: usize,
    in_chain: usize,
}

/// Machine 0's writes in each seed's chained world, each judged once its batch is written: replay
/// of what machine 0 holds applies it exactly when the causes it was built on said it would.
fn judged(seeds: impl IntoIterator<Item = u64>) -> Judged {
    let mut judged = Judged::default();
    for seed in seeds {
        let mut world = World::chained(20_261_121 + seed);
        let mut data = Object::new();
        data.insert("prefix".into(), Value::from("pr"));
        world.write(0, "init", None, data);
        for _ in 0..60 {
            world.next_batch();
            let built = std::mem::take(&mut world.built);
            if built.is_empty() {
                continue;
            }
            let state = replay(load(&world, &world.machines[0].known)).unwrap();
            for (at, causes) in built {
                let sequenced = !world.events[at].attempt;
                let id = world.id_of(at);
                let held_back =
                    causes.waits || (sequenced && (causes.broken || causes.predecessor_waits));
                let took = in_chain(&state, &id);
                assert_eq!(
                    took,
                    !held_back,
                    "seed {seed}: {} {id} built on {causes:?}",
                    super::str_of(world.events[at].value.get("op"))
                );
                let before: Vec<usize> = (0..at)
                    .filter(|at| world.folders[world.events[*at].machine] == MACHINES[0])
                    .filter(|at| !world.events[*at].attempt)
                    .collect();
                // the record before it is a placeholder, its op no text
                let placeholder = (before.last()).is_some_and(|last| {
                    world.events[*last]
                        .value
                        .get("op")
                        .is_some_and(|op| op.as_str().is_none())
                });
                let turned_back = causes.predecessor.is_some_and(|tail| {
                    (before.iter()).any(|at| EventId::parse(&world.id_of(*at)) > Some(tail))
                });
                judged.writes += 1;
                judged.turned_back += usize::from(turned_back && sequenced);
                judged.after_placeholder += usize::from(placeholder && sequenced);
                judged.predecessors += usize::from(causes.predecessor.is_some() && sequenced);
                judged.waits += usize::from(causes.waits);
                judged.predecessor_waits += usize::from(causes.predecessor_waits && sequenced);
                judged.broken += usize::from(causes.broken && sequenced);
                judged.in_chain += usize::from(took);
            }
        }
    }
    judged
}

#[test]
fn every_write_built_on_what_the_projection_names_joins_its_chain_as_it_said() {
    let judged = judged(0..40);
    assert!(judged.writes > 1_000, "{judged:?}");
    assert!(judged.in_chain > judged.writes / 3, "{judged:?}");
    assert!(judged.predecessors > 20, "{judged:?}");
    assert!(judged.waits > 20, "{judged:?}");
    assert!(judged.predecessor_waits > 0, "{judged:?}");
    assert!(judged.broken > 0, "{judged:?}");
    assert!(judged.turned_back > 0, "{judged:?}");
    assert!(judged.after_placeholder > 20, "{judged:?}");
}

#[test]
fn every_extension_past_a_journal_the_frontier_leaves_is_replays_answer_or_declined() {
    let tally = differential(0..40, HistoryKind::Chained);
    // what waits closes the projection to extension: most batches come to one replay makes whole
    assert!(tally.applied > 300, "{tally:?}");
    assert!(tally.deferred > 80, "{tally:?}");
    assert!(
        tally.past.get("chain break").is_some_and(|n| *n > 50),
        "{tally:?}"
    );
}

/// The same over many more seeds, eight threads at a time: `cargo test --release --package
/// beadroll-core -- --ignored many_chained`, as the nightly workflow's `seeds` job runs it.
#[test]
#[ignore = "minutes: run in release"]
fn many_chained_seeds() {
    const SEEDS: u64 = 2_000;
    const THREADS: u64 = 8;
    let counts: Vec<(usize, usize)> = std::thread::scope(|scope| {
        let runs: Vec<_> = (0..THREADS)
            .map(|n| {
                let seeds = n * SEEDS / THREADS..(n + 1) * SEEDS / THREADS;
                scope.spawn(move || {
                    let writes = judged(seeds.clone()).writes;
                    (writes, differential(seeds, HistoryKind::Chained).applied)
                })
            })
            .collect();
        runs.into_iter().map(|run| run.join().unwrap()).collect()
    });
    let writes: usize = counts.iter().map(|(writes, _)| writes).sum();
    let applied: usize = counts.iter().map(|(_, applied)| applied).sum();
    assert!(writes > 50_000 && applied > 15_000, "{writes} {applied}");
}
