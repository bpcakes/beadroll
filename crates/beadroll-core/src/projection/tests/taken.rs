//! Machine 0's store of a world taking in what the machines write, batch by batch, its projection
//! extended by each or declining it, every extension compared with projecting the whole journal;
//! and the projection that store last wrote back, extended by every batch since at once, as the
//! next command extends it when the writes before it left it as it was (roll-lqjj.36). Each is
//! kept three ways (roll-lqjj.43): written back as the store writes it, and at every write-back
//! with what every event had seen left as it is, holding events the state no longer names, or cut
//! down to those it names. Every extension of a projection read back is checked for what a
//! write-back may copy of it (roll-lqjj.44, [`marked`]).

use std::collections::BTreeMap;

use crate::json::{Object, Value};
use crate::replay::replay;

use super::super::{Extended, Projection, Record, named, project};
use super::awaiting::fnv;
use super::marked::{self, Records};
use super::{
    MACHINES, Rng, TextFile, World, WriteBack, answers_alike, edges, equal, flawed, load, owned,
    read_back, read_in, records, same, shown, whole,
};

/// What [`differential`] saw: the batches taken in, the merges among them, the batches that came to
/// a projection that could not be extended, and the local writes declined by one that could that are
/// not takeovers (by seed); with the extensions applied to a projection that held each kind of
/// integrity entry (by [`flawed::kinds`]), and the entries of each kind the batches brought; and
/// of the projection written back some batches before, the extensions by two batches or more at
/// once, and the most batches one took in: each of the projection written back as the store
/// writes it. Of the write-backs that left what every event had seen as it was, those that left
/// events the state no longer names, and the most such events one left; of those the store made,
/// those that left such events, and those that compacted them away. And by seed, the digest of what
/// every state replay made of its history, batch by batch, answers of its edges ([`edges::fold`]),
/// and what those states hold of the edges the generator draws for that ([`edges::drawn`]). And
/// the digest of what replay answers of every issue's ownership at the end of each seed's history
/// (`owned::answers`), and what those histories hold of it, with the extensions of a projection
/// read back that added to a history it had read in. And what the extensions of projections read
/// back changed ([`marked::Exercised`]).
#[derive(Default, Debug)]
pub(super) struct Tally {
    pub(super) applied: usize,
    pub(super) deferred: usize,
    pub(super) longest: usize,
    pub(super) merged: usize,
    pub(super) closed: usize,
    pub(super) declined: Vec<u64>,
    pub(super) past: BTreeMap<&'static str, usize>,
    pub(super) brought: BTreeMap<&'static str, usize>,
    pub(super) uncompacted: usize,
    pub(super) unnamed: usize,
    pub(super) left: usize,
    pub(super) cut: usize,
    pub(super) answers: Vec<(u64, u64)>,
    pub(super) drawn: edges::drawn::Exercised,
    pub(super) owned: u64,
    pub(super) seen: owned::Seen,
    pub(super) marked: marked::Exercised,
    pub(super) written: usize,
}

impl Tally {
    /// Counts what `read`, read back from `written` written back as `back` says, knows that its
    /// state no longer names.
    fn read_back(&mut self, written: &Projection, read: &Projection, back: WriteBack) {
        let mut kept = named(read.state());
        kept.extend(read.heads.iter().copied());
        let events = read.state.knowledge.events.keys();
        let unnamed = events.filter(|id| !kept.contains(*id)).count();
        match back {
            WriteBack::Uncompacted => {
                self.uncompacted += usize::from(unnamed > 0);
                self.unnamed = self.unnamed.max(unnamed);
            }
            WriteBack::Encoded => {
                self.left += usize::from(unnamed > 0);
                self.cut += usize::from(read.known() < written.known());
            }
            WriteBack::Compacted => assert_eq!(unnamed, 0),
        }
    }
}

/// Machine 0's store, of each seed's world, takes in 60 batches, each extending its projection or
/// declined; every extension must be what projecting the whole journal makes, and answer as replay
/// does of every event the state names, whichever way the projection was written back. A world
/// `flawed` has machines that also write what replay reports, and one whose identity was copied.
pub(super) fn differential(seeds: impl IntoIterator<Item = u64>, flawed: bool) -> Tally {
    let mut tally = Tally {
        owned: owned::BASIS,
        ..Tally::default()
    };
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
        let first = project(load(&world, &held)).unwrap();
        // the store, each way: its projection, extended by each batch, its text file, and what it
        // held as it was last read back, if it was since replay last made it
        let mut tracks = WriteBack::EVERY.map(|back| (first.clone(), TextFile::new(), back, None));
        // the store whose writes leave the projection to a later command, each way: what it wrote
        // back last, with its own text file, the batches taken in since, and draws of its own, so
        // that the world is the one it was before it
        let mut stored = WriteBack::EVERY.map(|back| Stored::new(&first, back, seed));
        let mut answers = edges::START;
        // the tracks' own draws: which texts they read in besides the batch's
        let mut coin = Rng(seed);
        for n in 0..60 {
            let batch = world.next_batch();
            let reading_back = world.rng.chance(50);
            held.extend(&batch);
            let full = project(load(&world, &held)).unwrap();
            let replayed = replay(load(&world, &held)).unwrap();
            assert!(same(full.state(), &replayed));
            assert!(answers_alike(&full, &replayed), "seed {seed}");
            shown::agree(&load(&world, &held), full.state());
            answers = edges::fold(answers, full.state(), n == 59);
            tally.drawn.count(full.state());
            for each in &mut stored {
                each.take(&world, &batch, &full, seed, &mut tally);
            }
            let local = batch.iter().all(|at| world.events[*at].machine == 0);
            let records = records(&world, &batch);
            // as a store does: the texts of the issues the batch is about read in first
            tracks = tracks.map(|(mut projection, mut file, back, mut held)| {
                // the figures are the store's
                let counted = back == WriteBack::Encoded;
                if reading_back {
                    let read = read_back(&projection, &mut file, back);
                    tally.read_back(&projection, &read, back);
                    held = Some(Records::of(&read));
                    projection = read;
                }
                let before = flawed::kinds(projection.state());
                let open = projection.extendable();
                tally.closed += usize::from(!open && counted);
                let about = projection.about(&records);
                read_in(projection.state_mut(), &file, Some(about.clone()));
                let histories = owned::histories(projection.state(), &about);
                let ahead = marked::ahead(&mut projection, &file, &mut coin, held.as_ref());
                // as the transaction that wrote them holds them (roll-lqjj.51)
                let writing = (local && counted).then(|| projection.clone());
                if counted {
                    for (kind, n) in flawed::kinds(full.state()) {
                        let new = n.saturating_sub(before.get(kind).copied().unwrap_or(0));
                        *tally.brought.entry(kind).or_default() += new;
                    }
                }
                let projection = match projection.extend(records.clone()).unwrap() {
                    Extended::Applied(extended) => {
                        assert!(
                            equal(&whole(&extended, &file), &full),
                            "seed {seed}, {back:?}"
                        );
                        let marks = &mut tally.marked;
                        marked::checked(held.as_ref(), ahead, &extended, &records, &about, marks);
                        tally.written += usize::from(as_written(writing, &records, &extended));
                        if counted {
                            for kind in before.into_keys() {
                                *tally.past.entry(kind).or_default() += 1;
                            }
                            tally.applied += 1;
                            tally.merged += usize::from(!local);
                            let added = owned::added(&histories, extended.state());
                            tally.seen.appended += usize::from(held.is_some() && added);
                        }
                        *extended
                    }
                    Extended::Replay => {
                        if counted && local && open && !world.takes_over(&batch) {
                            tally.declined.push(seed);
                        }
                        held = None; // replayed: nothing read back
                        full.clone()
                    }
                };
                (projection, file, back, held)
            });
        }
        tally.answers.push((seed, answers));
        tally.owned_alike(&world, &held, &mut tracks, seed);
    }
    tally
}

impl Tally {
    /// Folds in what replay answers of every issue's ownership at the end of `world`'s history and
    /// what that history holds of it, once each track's projection, read back, answers the same.
    fn owned_alike(
        &mut self,
        world: &World,
        held: &[usize],
        tracks: &mut [(Projection, TextFile, WriteBack, Option<Records>)],
        seed: u64,
    ) {
        let state = replay(load(world, held)).unwrap();
        let said = owned::answers(&state);
        for (projection, file, back, _) in tracks {
            let read = whole(&read_back(projection, file, *back), file);
            assert!(
                owned::answers(read.state()) == said,
                "seed {seed}, {back:?}"
            );
        }
        self.owned = fnv(self.owned, said.as_bytes());
        self.seen.count(world, &state);
    }
}

/// Whether `writing`, the projection a store's own write extends in its transaction, was there:
/// extended by `records` as the transaction emitted them, read from no file, it must write back
/// the bytes `read`, extended by the same records read from the journal, writes (roll-lqjj.51).
fn as_written(writing: Option<Projection>, records: &[Record], read: &Projection) -> bool {
    let Some(writing) = writing else {
        return false;
    };
    let emitted = (records.iter().cloned())
        .map(|record| Record {
            origin: String::new(),
            ..record
        })
        .collect();
    let Extended::Applied(written) = writing.extend(emitted).unwrap() else {
        panic!("a transaction's records declined where the journal's were applied");
    };
    let (ours, theirs) = (written.encode(0).0, read.encode(0).0);
    assert!(
        ours.state == theirs.state && ours.index == theirs.index && ours.texts == theirs.texts,
        "a write's write-back differs from a read's"
    );
    true
}

/// The projection a store wrote back last, what it held as it was read back, its text file, and
/// what came after it.
struct Stored {
    projection: Projection,
    read: Records,
    file: TextFile,
    /// The events taken in since it was written back, as they came.
    since: Vec<usize>,
    /// How many batches they came in.
    batches: usize,
    coin: Rng,
    /// How it is written back.
    back: WriteBack,
}

impl Stored {
    /// The store's first projection, `first`, written back as `back` says, with draws of its own.
    fn new(first: &Projection, back: WriteBack, seed: u64) -> Stored {
        let mut file = TextFile::new();
        let projection = read_back(first, &mut file, back);
        Stored {
            read: Records::of(&projection),
            projection,
            file,
            since: Vec::new(),
            batches: 0,
            coin: Rng(seed),
            back,
        }
    }

    /// Takes `batch` in: the projection written back is extended by every record since at once, as
    /// a store reads them, and must be what projecting the whole journal makes (`full`); it is then
    /// written back now and then, as a read or a write after many records does, or, declined, made
    /// whole and written back, as a store replays.
    fn take(
        &mut self,
        world: &World,
        batch: &[usize],
        full: &Projection,
        seed: u64,
        tally: &mut Tally,
    ) {
        self.since.extend(batch);
        self.batches += 1;
        let records = read_after(world, &self.since);
        let about = self.projection.about(&records);
        let mut extending = self.projection.clone();
        read_in(extending.state_mut(), &self.file, Some(about.clone()));
        let before = owned::histories(extending.state(), &about);
        let ahead = Records::of(&extending);
        let written = match extending.extend(records.clone()).unwrap() {
            Extended::Applied(extended) => {
                assert!(
                    equal(&whole(&extended, &self.file), full),
                    "seed {seed}: {} batches at once",
                    self.batches
                );
                let marks = &mut tally.marked;
                marked::check(&self.read, &ahead, &extended, &records, &about, marks);
                if self.back == WriteBack::Encoded {
                    tally.seen.appended += usize::from(owned::added(&before, extended.state()));
                }
                if self.batches > 1 && self.back == WriteBack::Encoded {
                    tally.deferred += 1;
                    tally.longest = tally.longest.max(self.batches);
                }
                self.coin.chance(30).then_some(*extended)
            }
            Extended::Replay => Some(full.clone()),
        };
        if let Some(written) = written {
            self.projection = read_back(&written, &mut self.file, self.back);
            self.read = Records::of(&self.projection);
            tally.read_back(&written, &self.projection, self.back);
            (self.since, self.batches) = (Vec::new(), 0);
        }
    }
}

/// The records of `since` as a store reads them after its projection: each machine's folder's
/// segment records in the order they were appended, the folders by name, then the attempt files,
/// by name.
fn read_after(world: &World, since: &[usize]) -> Vec<Record> {
    let mut order: Vec<usize> = Vec::with_capacity(since.len());
    for folder in MACHINES {
        let here = |at: &&usize| {
            let written = &world.events[**at];
            !written.attempt && world.folders[written.machine] == folder
        };
        order.extend(since.iter().filter(here));
    }
    let mut attempts: Vec<(&str, String, usize)> = (since.iter())
        .filter(|at| world.events[**at].attempt)
        .map(|at| {
            (
                world.folders[world.events[*at].machine],
                world.id_of(*at),
                *at,
            )
        })
        .collect();
    attempts.sort();
    order.extend(attempts.into_iter().map(|(_, _, at)| at));
    records(world, &order)
}
