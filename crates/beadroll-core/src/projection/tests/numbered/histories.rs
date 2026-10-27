//! Seeded histories of numbered keys, written by three machines that often mint the same keys
//! offline (roll-lqjj.46): replay, and a projection taking each batch in, read back each way or
//! not, pick what replay picked before copies skipped runs.

use crate::json::{Object, Value};
use crate::replay::replay;
use crate::state::{Rename, asked, used};

use super::super::super::{Extended, project};
use super::super::awaiting::{by_key, creating, fnv, on, rendered};
use super::super::{
    MACHINES, TextFile, World, WriteBack, answers_alike, equal, load, read_back, read_in, records,
    same, whole,
};

/// The stems the seeded histories number keys under: one is itself a key of another, one has a dot
/// of its own.
const STEMS: [&str; 3] = ["pr-n", "pr-n.3", "pr-m"];

/// The largest number an `i128` holds, without its last digit.
const NEAR_MOST: &str = "17014118346046923173168730371588410572";

/// A key of the seeded histories: mostly a small number, so that machines mint the same keys and
/// leave gaps, now and then with a leading zero, about the largest an `i128` holds, or 9 forty
/// times, which counting on carries.
fn numbered(world: &mut World) -> String {
    let stem = STEMS[world.rng.below(STEMS.len())];
    let n = world.rng.below(14);
    let number = match world.rng.below(25) {
        0 | 1 => format!("0{n}"),
        2 => format!("{NEAR_MOST}{}", 5 + world.rng.below(5)),
        3 => "9".repeat(40),
        _ => n.to_string(),
    };
    format!("{stem}.{number}")
}

/// The keys the import that created `issue` waits for, as machine `m` knows it.
fn imported(world: &World, m: usize, issue: &str) -> Vec<String> {
    (world.machines[m].known.iter())
        .map(|at| &world.events[*at].value)
        .filter(|value| value.get("id").and_then(Value::as_str) == Some(issue))
        .filter_map(|value| value.get("data")?.get("dangling")?.as_array())
        .flatten()
        .filter_map(|dep| Some(dep.get("depends_on_id")?.as_str()?.to_owned()))
        .collect()
}

/// One thing machine `m` does in a history of numbered keys: mostly creates, many of them imports
/// that wait for numbered keys, and those taken back by key.
fn act(world: &mut World, m: usize) {
    let (issues, _) = world.issues(m);
    let roll = world.rng.below(100);
    let issue = world.rng.pick(&issues).cloned();
    let Some(issue) = issue.filter(|_| roll >= 55) else {
        let made = numbered(world);
        let mut named = Vec::new();
        if world.rng.chance(40) {
            for _ in 0..1 + world.rng.below(3) {
                let kind = [Some("blocks"), Some("related"), None][world.rng.below(3)];
                named.push(on(&numbered(world), kind, &[]));
            }
        }
        world.write(m, "create", None, creating(&made, named));
        return;
    };
    let mut data = Object::new();
    let op = if roll < 85 {
        let waited = imported(world, m, &issue);
        let key = match world.rng.pick(&waited).cloned() {
            Some(key) if world.rng.chance(80) => key,
            _ => numbered(world),
        };
        data = by_key(&key, None);
        "dep_remove"
    } else {
        let mut fields = Object::new();
        fields.insert("title".into(), Value::from(format!("t{roll}")));
        data.insert("fields".into(), Value::from(fields));
        "set"
    };
    world.write(m, op, Some(issue), data);
}

/// What machine 0's store takes in next: its own writes, or a merge of what the others wrote
/// meanwhile, often offline, so that they mint the same keys.
fn next_batch(world: &mut World) -> Vec<usize> {
    loop {
        let m = world.rng.below(MACHINES.len());
        if m == 0 && world.rng.chance(50) {
            let before = world.events.len();
            for _ in 0..1 + world.rng.below(3) {
                act(world, 0);
            }
            return (before..world.events.len()).collect();
        }
        if m == 0 {
            return world.sync(0);
        }
        if world.rng.chance(40) {
            world.sync(m);
        }
        for _ in 0..1 + world.rng.below(4) {
            act(world, m);
        }
        world.sync(m);
    }
}

/// What the seeded histories exercise.
#[derive(Debug, Default)]
struct Exercised {
    /// Copies renamed, those that counted past three numbers or more, and batches a projection
    /// took rather than replaying.
    renames: usize,
    far: usize,
    applied: usize,
    /// Keys asked of, and those claimed by an import that waits alone.
    asked: usize,
    awaited: usize,
    /// What runs did, every way the states were made.
    used: used::Used,
}

/// How far a rename counted, when both its numbers are short.
fn counted(rename: &Rename) -> Option<u64> {
    let number = |key: &str| key.rsplit_once('.')?.1.parse::<u64>().ok();
    number(&rename.to)?.checked_sub(number(&rename.from)?)
}

/// The digest [`rendered`] gives of every seed's whole history, as replay made it at 3ae3604,
/// before copies skipped runs: replay must make the same of them.
const BEFORE_THE_RUNS: u64 = 0xe7ed_ea64_8d13_2e1f;

#[test]
fn histories_of_numbered_keys_agree_every_way_and_with_replay_before_the_runs() {
    let mut digest = 0xcbf2_9ce4_8422_2325;
    let mut exercised = Exercised::default();
    used::taken();
    for seed in 0..20 {
        let mut world = World::new(20_261_026 + seed);
        let mut data = Object::new();
        data.insert("prefix".into(), Value::from("pr"));
        world.write(0, "init", None, data);
        let mut held: Vec<usize> = vec![0];
        let first = project(load(&world, &held)).unwrap();
        // each way it can be written back (roll-lqjj.43)
        let mut tracks = WriteBack::EVERY.map(|back| (first.clone(), TextFile::new(), back));
        for _ in 0..40 {
            let batch = next_batch(&mut world);
            let reading_back = world.rng.chance(50);
            held.extend(&batch);
            let full = project(load(&world, &held)).unwrap();
            let replayed = replay(load(&world, &held)).unwrap();
            assert!(same(full.state(), &replayed) && answers_alike(&full, &replayed));
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
        asked::taken();
        let everything: Vec<usize> = (0..world.events.len()).collect();
        let state = replay(load(&world, &everything)).unwrap();
        for ask in asked::taken() {
            exercised.asked += 1;
            exercised.awaited += usize::from(ask.awaited);
        }
        digest = fnv(digest, rendered(&state).as_bytes());
        exercised.renames += state.renames().len();
        exercised.far += (state.renames().iter())
            .filter(|rename| counted(rename).is_some_and(|n| n >= 3))
            .count();
    }
    exercised.used = used::taken();
    let seen = &exercised;
    assert!(
        seen.renames > 300 && seen.far > 50 && seen.applied > 300 && seen.awaited > 50,
        "{exercised:?}"
    );
    // copies skipped runs, imports were taken back from inside them, and every key a copy took,
    // replayed or extended, was the key the rule counting one number at a time takes
    let used = seen.used;
    assert!(
        used.skipped > 1_000 && used.split > 50 && used.compared > 3 * seen.renames,
        "{exercised:?}"
    );
    assert_eq!(digest, BEFORE_THE_RUNS, "{digest:#x}, {exercised:?}");
}
