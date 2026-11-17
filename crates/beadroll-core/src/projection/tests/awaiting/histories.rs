//! Seeded histories heavy in imports that wait, made every way a state is made, agree with each
//! other and with what replay made of them before the index (`BEFORE_THE_INDEX`).

use super::super::super::{Extended, project};
use super::super::{
    MACHINES, TextFile, World, WriteBack, answers_alike, equal, load, read_back, read_in, records,
    same, whole,
};
use super::{by_key, creating, edges, fnv, on, rendered};

use crate::id::EventId;
use crate::json::{Object, Value};
use crate::projection::tests::edges::AnswerCoverage;
use crate::replay::replay;
use crate::state::{State, asked};
use crate::time::event_time;

/// How many keys the seeded histories create, `pr-k0` onwards, and how many they name: the last
/// five never have an issue.
const MADE: usize = 25;
const NAMED: usize = 30;

/// Key number `n` of the seeded histories.
fn key(n: usize) -> String {
    format!("pr-k{n}")
}

/// The types an import names: the usual three, none, and a falsy one, both of which are blocks.
const KINDS: [Option<&str>; 5] = [
    Some("blocks"),
    Some("related"),
    Some("parent-child"),
    None,
    Some(""),
];

/// One thing machine `m` does in a history heavy in imports that wait: mostly creates, many of them
/// imports naming keys that are not issues yet (their own among them), and removals by key and by
/// target.
fn act(world: &mut World, m: usize) {
    let (issues, _) = world.issues(m);
    let roll = world.rng.below(100);
    let issue = world.rng.pick(&issues).cloned();
    let mut data = Object::new();
    let Some(issue) = issue.filter(|_| roll >= 35) else {
        let made = key(world.rng.below(MADE));
        let mut named = Vec::new();
        if world.rng.chance(70) {
            for n in 0..1 + world.rng.below(4) {
                let target = if world.rng.chance(10) {
                    made.clone()
                } else {
                    key(world.rng.below(NAMED))
                };
                let kind = KINDS[world.rng.below(KINDS.len())];
                let mut extra = Vec::new();
                let tag = format!("m{roll}.{n}");
                if world.rng.chance(30) {
                    extra.push(("metadata", tag.as_str()));
                }
                if world.rng.chance(20) {
                    extra.push(("created_by", "importer"));
                }
                if world.rng.chance(10) {
                    extra.push(("thread_id", "t9"));
                }
                named.push(on(&target, kind, &extra));
            }
        }
        world.write(m, "create", None, creating(&made, named));
        return;
    };
    let kind = |world: &mut World| ["blocks", "related", "parent-child"].get(world.rng.below(4));
    let op = match roll {
        35..=59 => {
            data = by_key(&key(world.rng.below(NAMED)), kind(world).copied());
            "dep_remove"
        }
        60..=74 => {
            let to = world.rng.pick(&issues).cloned().unwrap_or_default();
            data.insert("to".into(), Value::from(to));
            data.insert(
                "type".into(),
                kind(world).map_or(Value::Null, |k| Value::from(*k)),
            );
            "dep_remove"
        }
        75..=84 => {
            let to = world.rng.pick(&issues).cloned().unwrap_or_default();
            data.insert("to".into(), Value::from(to));
            data.insert("type".into(), Value::from("blocks"));
            data.insert("meta".into(), Value::from(Object::new()));
            "dep_add"
        }
        _ => {
            let mut fields = Object::new();
            fields.insert("title".into(), Value::from(format!("t{roll}")));
            data.insert("fields".into(), Value::from(fields));
            "set"
        }
    };
    world.write(m, op, Some(issue), data);
}

/// What machine 0's store takes in next, as [`World::next_batch`] chooses it, with [`act`].
fn next_batch(world: &mut World) -> Vec<usize> {
    loop {
        let m = world.rng.below(MACHINES.len());
        if m == 0 && world.rng.chance(60) {
            let before = world.events.len();
            for _ in 0..1 + world.rng.below(3) {
                act(world, 0);
            }
            return (before..world.events.len()).collect();
        }
        if m == 0 {
            return world.sync(0);
        }
        if world.rng.chance(70) {
            world.sync(m);
        }
        for _ in 0..1 + world.rng.below(3) {
            act(world, m);
        }
        world.sync(m);
    }
}

/// What the seeded histories exercise, counted in what replay makes of them.
#[derive(Debug, Default)]
struct Exercised {
    /// Edges an import brought to an issue made after the importing one: it waited for the key.
    waited: usize,
    /// Entries still waiting, and those among them that name their own issue's key.
    waiting: usize,
    own: usize,
    /// Copies renamed, and batches a projection took rather than replaying.
    renames: usize,
    applied: usize,
    /// Keys asked of while copies were renamed, those claimed by a dependency that waits alone, and
    /// those asked while an entry taken out of a list held its place.
    asked: usize,
    awaited: usize,
    mid_batch: usize,
}

impl Exercised {
    fn count(&mut self, state: &State) {
        let places: std::collections::HashMap<EventId, usize> = (state.issues().enumerate())
            .map(|(at, issue)| (issue.uid(), at))
            .collect();
        for issue in state.issues() {
            let created = event_time(issue.uid().millis());
            for (edge, meta) in issue.deps().iter() {
                let meta = edges::shown(meta);
                let imported = meta.get("created_at").and_then(Value::as_str) == Some(&*created)
                    || meta.get("created_by").and_then(Value::as_str) == Some("importer");
                let later = (edge.to().id().and_then(|to| places.get(&to)))
                    .is_some_and(|to| *to > places[&issue.uid()]);
                self.waited += usize::from(imported && later);
            }
            self.waiting += issue.dangling().len();
            self.own += (issue.dangling().iter())
                .filter(|dep| dep.get("depends_on_id").and_then(Value::as_str) == Some(issue.key()))
                .count();
        }
        self.renames += state.renames().len();
    }

    /// Counts the asks made since the last were taken.
    fn asks(&mut self) {
        for ask in asked::taken() {
            self.asked += 1;
            self.awaited += usize::from(ask.awaited);
            self.mid_batch += usize::from(ask.mid_batch);
        }
    }
}

/// Fails unless the finished `state` answers of every key it holds or names, of every key the
/// histories make or name, and of the keys their renames tried, whether it claims it as collecting
/// every key does.
fn claims_as_collected(state: &State) {
    let collected = state.claimed_keys();
    let mut keys: Vec<String> = collected.iter().map(|key| (*key).to_owned()).collect();
    keys.extend((0..NAMED + 5).map(key));
    for rename in state.renames() {
        keys.extend([rename.from.clone(), format!("{}0", rename.from)]);
    }
    for key in &keys {
        let claimed = state.claims_key(key);
        assert_eq!(claimed, collected.contains(key.as_str()), "{key}");
    }
}

/// The digest [`rendered`] gives of every seed's whole history, as replay made it at cdf311f,
/// before the index: replay must make the same of them.
const BEFORE_THE_INDEX: u64 = 0xbffd_7bce_ab55_9b7c;

#[test]
fn histories_heavy_in_imports_that_wait_agree_every_way_and_with_replay_before_the_index() {
    let mut digest = 0xcbf2_9ce4_8422_2325;
    let mut exercised = Exercised::default();
    let mut answers = Vec::new();
    for seed in 0..30 {
        let mut answered = edges::START;
        let mut world = World::new(20_261_015 + seed);
        let mut data = Object::new();
        data.insert("prefix".into(), Value::from("pr"));
        world.write(0, "init", None, data);
        let mut held: Vec<usize> = vec![0];
        let first = project(load(&world, &held)).unwrap();
        // each way it can be written back (roll-lqjj.43), the figures the store's
        let mut tracks = WriteBack::EVERY.map(|back| (first.clone(), TextFile::new(), back));
        for n in 0..50 {
            let batch = next_batch(&mut world);
            let reading_back = world.rng.chance(50);
            held.extend(&batch);
            let full = project(load(&world, &held)).unwrap();
            let replayed = replay(load(&world, &held)).unwrap();
            assert!(same(full.state(), &replayed) && answers_alike(&full, &replayed));
            answered = edges::fold(
                answered,
                full.state(),
                if n == 49 {
                    AnswerCoverage::EveryWay
                } else {
                    AnswerCoverage::Direct
                },
            );
            let records = records(&world, &batch);
            tracks = tracks.map(|(mut projection, mut file, back)| {
                if reading_back {
                    projection = read_back(&projection, &mut file, back);
                }
                let about = projection.about(&records);
                read_in(projection.state_mut(), &file, Some(about));
                let projection = match projection.extend(records.clone()).unwrap() {
                    Extended::Applied(extended) => {
                        assert!(
                            equal(&whole(&extended, &file), &full),
                            "seed {seed}, {back:?}"
                        );
                        exercised.applied += usize::from(back == WriteBack::Encoded);
                        *extended
                    }
                    Extended::Replay => full.clone(),
                };
                (projection, file, back)
            });
        }
        let everything: Vec<usize> = (0..world.events.len()).collect();
        let state = replay(load(&world, &everything)).unwrap();
        exercised.asks();
        claims_as_collected(&state);
        asked::taken();
        digest = fnv(digest, rendered(&state).as_bytes());
        exercised.count(&state);
        answers.push((seed, answered));
    }
    let seen = &exercised;
    assert!(
        seen.waited > 500
            && seen.waiting > 500
            && seen.own > 50
            && seen.renames > 500
            && seen.applied > 500,
        "{exercised:?}"
    );
    // Every rename asked as collecting every key would, in the batch an entry was taken out in and
    // of keys only an import waits for among them (`asked`, which fails otherwise).
    assert!(
        seen.asked > seen.renames && seen.awaited > 100 && seen.mid_batch > 500,
        "{exercised:?}"
    );
    assert_eq!(digest, BEFORE_THE_INDEX, "{digest:#x}");
    edges::pinned("awaiting", &answers);
}
