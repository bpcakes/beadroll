//! Histories with everything replay reports in them (roll-lqjj.28): machines other than the store
//! under test also write what replay sets aside, each kind of integrity entry, and one of them is
//! a copy of another, writing to its journal without building on it. A projection holding any of
//! them is still extended only as replay would apply the batch, or declines.

use std::collections::BTreeMap;

use crate::json::{Object, Value};
use crate::state::{Integrity, State};

use super::taken::{Tally, differential};
use super::{MACHINES, Machine, World, Written, str_of};

impl World {
    /// A world of [`MACHINES`] and a fourth machine, a copy of the second's identity: it writes
    /// to the second's journal, from what it knows itself, as a machine whose identity was copied
    /// does; and machines other than the first also write what replay reports ([`World::flaw`]).
    pub(super) fn flawed(seed: u64) -> World {
        let mut world = World::new(seed);
        world.machines.push(Machine::default());
        world.folders.push(MACHINES[1]);
        world.flawed = true;
        world
    }

    /// One thing machine `m` (never the store under test) writes that replay reports: an unknown
    /// operation, a payload or an event that is not valid, an unreadable line, a second content of
    /// an id, an event in its journal written by another machine (and that machine's own record of
    /// it later, of the same content or another), a takeover of a claim its writer had not seen,
    /// of something that is not a claim on its issue, or of an id no event has yet (and that event
    /// later, a claim on the same issue).
    pub(super) fn flaw(&mut self, m: usize) {
        let (issues, claims) = self.issues(m);
        let Some(issue) = self.rng.pick(&issues).cloned() else {
            return self.act(m);
        };
        let mut data = Object::new();
        match self.rng.below(10) {
            0 => drop(self.write(m, "future_op", Some(issue), data)),
            1 => {
                data.insert("fields".into(), Value::from(5));
                drop(self.write(m, "set", Some(issue), data));
            }
            2 => {
                self.write(m, "comment", Some(issue), data);
                self.rewrite(self.events.len() - 1, "data", Value::from(5));
            }
            3 => {
                let mut line = Object::new();
                line.insert("op".into(), Value::from("set"));
                self.push(m, Value::from(line), false);
            }
            4 => {
                let own: Vec<usize> = (self.machines[m].known.iter().copied())
                    .filter(|at| self.folders[self.events[*at].machine] == self.folders[m])
                    .filter(|at| !self.events[*at].attempt && !self.id_of(*at).is_empty())
                    .collect();
                if let Some(&at) = self.rng.pick(&own) {
                    let value = self.events[at].value.clone();
                    let changed = self.rng.chance(70);
                    self.push(m, if changed { altered(&value) } else { value }, false);
                }
            }
            5 => {
                let others: Vec<&str> = (MACHINES[1..].iter().copied())
                    .filter(|folder| *folder != self.folders[m])
                    .collect();
                let writer = self.rng.pick(&others).copied().unwrap_or(MACHINES[2]);
                data.insert("text".into(), Value::from("forged"));
                self.write(m, "comment", Some(issue), data);
                self.rewrite(self.events.len() - 1, "replica", Value::from(writer));
            }
            6 => {
                // a record this machine knows, written in its name in another machine's journal
                let forged: Vec<usize> = (self.machines[m].known.iter().copied())
                    .filter(|at| {
                        let written = &self.events[*at];
                        str_of(written.value.get("replica")) == self.folders[m]
                            && self.folders[written.machine] != self.folders[m]
                    })
                    .collect();
                if let Some(&at) = self.rng.pick(&forged) {
                    let mut value = self.events[at].value.clone();
                    if self.rng.chance(50) {
                        // its own content: built on its own journal, as it writes
                        let deps = self.heads(m).into_iter().map(Value::from).collect();
                        value = altered(&value);
                        value = with(&value, "deps", Value::Array(deps));
                    }
                    self.push(m, value, false);
                }
            }
            7 => {
                // a claim on the issue this machine has not taken in
                let unseen: Vec<usize> = (0..self.events.len())
                    .filter(|at| !self.machines[m].known.contains(at))
                    .filter(|at| {
                        let value = &self.events[*at].value;
                        value.get("op").and_then(Value::as_str) == Some("claim")
                    })
                    .collect();
                if let Some(&at) = self.rng.pick(&unseen) {
                    let on = str_of(self.events[at].value.get("issue"));
                    data.insert("takeover".into(), Value::from(self.id_of(at)));
                    data.insert("mode".into(), Value::from("online"));
                    self.write(m, "claim", Some(on), data);
                }
            }
            8 => {
                // the issue's create, or a claim on another issue
                let named = match self.rng.pick(&claims) {
                    Some((on, held)) if *on != issue && self.rng.chance(50) => held.clone(),
                    _ => issue.clone(),
                };
                data.insert("takeover".into(), Value::from(named));
                data.insert("mode".into(), Value::from("online"));
                self.write(m, "claim", Some(issue), data);
            }
            _ => {
                if !self.promised.is_empty() && self.rng.chance(50) {
                    let at = self.rng.below(self.promised.len());
                    let (on, id) = self.promised.swap_remove(at);
                    self.next_id = Some(id);
                    data.insert("mode".into(), Value::from("online"));
                    self.write(m, "claim", Some(on), data);
                } else {
                    let id = self.mint();
                    self.promised.push((issue.clone(), id.clone()));
                    data.insert("takeover".into(), Value::from(id));
                    data.insert("mode".into(), Value::from("online"));
                    self.write(m, "claim", Some(issue), data);
                }
            }
        }
    }

    /// Machine `m` appends `value` to its journal (or, `attempt`, writes it as an attempt file).
    fn push(&mut self, m: usize, value: Value, attempt: bool) {
        self.events.push(Written {
            value,
            machine: m,
            attempt,
        });
        self.learn(m, self.events.len() - 1);
    }

    /// The event written at `at`, with `key` set to `value`.
    fn rewrite(&mut self, at: usize, key: &str, value: Value) {
        self.events[at].value = with(&self.events[at].value, key, value);
    }
}

/// `event` with `key` set to `value`.
fn with(event: &Value, key: &str, value: Value) -> Value {
    let mut object = event.as_object().cloned().unwrap_or_default();
    object.insert(key.into(), value);
    Value::from(object)
}

/// `event` with another content under its id.
fn altered(event: &Value) -> Value {
    with(event, "note", Value::from("another content"))
}

/// How many integrity entries of each kind `state` holds.
pub(super) fn kinds(state: &State) -> BTreeMap<&'static str, usize> {
    let mut out = BTreeMap::new();
    for entry in &state.integrity {
        *out.entry(kind(entry)).or_default() += 1;
    }
    out
}

/// What kind of integrity entry `entry` is.
fn kind(entry: &Integrity) -> &'static str {
    let problem = entry.problem.as_str();
    if entry.event.is_none() {
        "unreadable line"
    } else if problem.contains("found only in") {
        "record in another machine's journal"
    } else if problem.contains("data is not an object") {
        "invalid event"
    } else if problem.contains("different contents") {
        "quarantined duplicate"
    } else if problem.contains("chain broken") {
        "chain break"
    } else if problem.contains("had not seen") {
        "takeover of an unseen claim"
    } else if problem.starts_with("takeover") {
        "takeover of no claim on its issue"
    } else if problem.contains("unknown operation") {
        "unknown operation"
    } else {
        "invalid payload"
    }
}

/// The kinds every flawed history must have had extensions applied past.
const EVERY_KIND: [&str; 9] = [
    "chain break",
    "invalid event",
    "invalid payload",
    "quarantined duplicate",
    "record in another machine's journal",
    "takeover of an unseen claim",
    "takeover of no claim on its issue",
    "unknown operation",
    "unreadable line",
];

/// Seeds whose histories fail when one of the three things that keep the projection from being
/// extended is dropped: a takeover of an id no event has (90, 182), a record of an id an integrity
/// entry names (44), and an id with several contents whose causes loop back onto it (1451).
const KEEPS_OUT: [u64; 4] = [44, 90, 182, 1451];

#[test]
fn every_extension_past_an_integrity_entry_is_replays_answer_or_declined() {
    let tally = differential((0..40).chain(KEEPS_OUT), true);
    assert!(
        tally.declined.is_empty(),
        "local writes declined in seeds {:?}",
        tally.declined
    );
    for kind in EVERY_KIND {
        assert!(tally.past.get(kind).is_some_and(|n| *n > 0), "{tally:?}");
        assert!(tally.brought.get(kind).is_some_and(|n| *n > 0), "{tally:?}");
    }
    assert!(tally.deferred > 100, "{tally:?}");
    assert!(tally.uncompacted > 200, "{tally:?}");
    super::edges::pinned("flawed", &tally.answers);
    assert!(tally.seen.more_than(5), "{:?}", tally.seen);
    assert_eq!(tally.owned, super::owned::FLAWED, "{:#x}", tally.owned);
}

/// The same over many more seeds, eight threads at a time: `cargo test --release --package
/// beadroll-core -- --ignored many_seeds`.
#[test]
#[ignore = "minutes: run in release"]
fn many_seeds_past_integrity_entries() {
    const SEEDS: u64 = 10_000;
    const THREADS: u64 = 8;
    let tallies: Vec<Tally> = std::thread::scope(|scope| {
        let runs: Vec<_> = (0..THREADS)
            .map(|n| {
                let seeds = n * SEEDS / THREADS..(n + 1) * SEEDS / THREADS;
                scope.spawn(move || differential(seeds, true))
            })
            .collect();
        runs.into_iter().map(|run| run.join().unwrap()).collect()
    });
    let mut tally = Tally::default();
    for each in tallies {
        tally.applied += each.applied;
        tally.merged += each.merged;
        tally.closed += each.closed;
        tally.deferred += each.deferred;
        tally.uncompacted += each.uncompacted;
        tally.unnamed = tally.unnamed.max(each.unnamed);
        tally.left += each.left;
        tally.cut += each.cut;
        tally.longest = tally.longest.max(each.longest);
        tally.declined.extend(each.declined);
        for (kind, n) in each.past {
            *tally.past.entry(kind).or_default() += n;
        }
        for (kind, n) in each.brought {
            *tally.brought.entry(kind).or_default() += n;
        }
    }
    println!("{tally:#?}");
    assert!(tally.declined.is_empty(), "{:?}", tally.declined);
}
