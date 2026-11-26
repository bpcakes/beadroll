//! A projection extended by every batch a store takes in, local writes and merges alike, is the
//! state replay makes of the whole journal, or declines: on histories of machines that write, sync
//! through a remote and race (colliding keys, concurrent edits, claims, takeovers, imports that
//! wait for a key).

pub(crate) mod awaiting;
mod causes;
mod chained;
mod compacted;
mod copied;
mod edges;
mod fields;
mod finished;
mod flawed;
mod followed;
mod forged;
pub(crate) mod hostile;
mod known;
mod lines;
mod marked;
mod numbered;
pub(crate) mod owned;
mod reported;
mod shown;
mod taken;
mod texts_once;
mod world;

use taken::differential;
pub(crate) use world::history;
use world::{MACHINES, Machine, World, Written, load, records, str_of};

use super::parts::Copying;
use super::{Appended, Causes, Extended, Projection, decode_state};
use crate::id::EventId;
use crate::projection::parts::Compaction;
use crate::state::{KeySpace, State};

/// Whether generated histories include malformed or conflicting records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HistoryKind {
    /// Only records replay applies.
    Valid,
    /// Records replay reports too, and a machine whose identity was copied.
    Flawed,
    /// Flawed, and journals whose last record the frontier does not reach ([`World::chained`]),
    /// machine 0 building on what its projection names.
    Chained,
}

/// The seeded generator the tests of the crate draw from: the same draws on every run.
pub(crate) use beadroll_test_support::values::Rng;

impl Projection {
    /// The issues this track would load for a batch shared across the differential tests.
    fn about(&self, batch: &Appended) -> Vec<EventId> {
        batch
            .readable()
            .map_or_else(Vec::new, |batch| batch.about(self))
    }

    /// Applies a batch with the texts the test chose to load, including intentionally missing ones.
    pub(crate) fn extend(self, records: impl Into<Appended>) -> Result<Extended, KeySpace> {
        match records.into().into_readable() {
            Some(batch) => self.extend_readable(batch),
            None => Ok(Extended::Replay),
        }
    }
}

/// Whether two states answer alike: everything replay builds, and what a state answers with of its
/// events (who wrote what a register holds, where machines' journals broke).
fn same(a: &State, b: &State) -> bool {
    let conflicts = |st: &State| -> Vec<String> {
        (st.issues.values())
            .map(|issue| format!("{:?}", st.conflicts(issue.hydrated().unwrap())))
            .collect()
    };
    conflicts(a) == conflicts(b)
        && a.chain_breaks() == b.chain_breaks()
        && a.prefix == b.prefix
        && a.name == b.name
        && a.issues.iter().eq(b.issues.iter())
        && a.by_key.iter().eq(b.by_key.iter())
        && a.renames == b.renames
        && a.integrity == b.integrity
        && a.pending == b.pending
        && a.orphans == b.orphans
        && a.comment_count == b.comment_count
        && a.last_by_thread.iter().eq(b.last_by_thread.iter())
        && a.children == b.children
        && awaiting(a).is_some_and(|listed| Some(listed) == awaiting(b))
}

/// What `state` lists as waiting, by place: where an entry is listed at a place that does not hold
/// it, that place is `None`, which no state agrees with.
fn awaiting(state: &State) -> Option<Vec<(String, EventId, usize)>> {
    (state.awaiting.listed(&state.issues).into_iter())
        .map(|(key, uid, at)| Some((key, uid, at?)))
        .collect()
}

/// What a store's text file holds: the texts every projection read back placed, appended.
type TextFile = Vec<u8>;

/// Reads into `state` the texts it has there of `uids`, or of every issue, from `file`, as a store
/// reads them in.
fn read_in(state: &mut State, file: &[u8], uids: Option<Vec<EventId>>) {
    let wanted = match uids {
        Some(uids) => state.wanted_texts(uids),
        None => state.all_wanted_texts(),
    };
    for (uid, place) in wanted {
        let at = usize::try_from(place.at).unwrap();
        let bytes = &file[at..at + place.len as usize];
        state.read_texts(uid, place, bytes).unwrap();
    }
}

/// `projection` with every text it has there read in from `file`.
fn whole(projection: &Projection, file: &[u8]) -> Projection {
    let mut whole = projection.clone();
    read_in(whole.state_mut(), file, None);
    whole
}

/// How a track of the differential tests writes its projection back (roll-lqjj.43): as
/// [`Projection::encode`] does, or at every write-back with what every event had seen left as it
/// is, or at every write-back cut down to the events the state names and the frontier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WriteBack {
    Encoded,
    Uncompacted,
    Compacted,
}

impl WriteBack {
    /// Every way, the one the store takes first.
    pub(super) const EVERY: [WriteBack; 3] = [
        WriteBack::Encoded,
        WriteBack::Uncompacted,
        WriteBack::Compacted,
    ];

    /// Whether writing `projection` back cuts what every event had seen down to the events the
    /// state names and the frontier: as the store writes it, once the events taken in since it was
    /// last compacted are more than an eighth of those it knew then.
    fn compacts(self, projection: &Projection) -> bool {
        let since = projection.known() - projection.compacted();
        match self {
            WriteBack::Encoded => since * 8 > projection.compacted(),
            WriteBack::Compacted => true,
            WriteBack::Uncompacted => false,
        }
    }
}

/// The projection read back from its bytes, as the next command reads it, written back as `back`
/// says, its new texts appended to `file` and every issue's texts there. Read back with some
/// issues' texts read in alone, as a command that prints those issues whole reads it, every issue
/// answers who holds it as it did, and those read in their ownership history too
/// (`owned::read_alike`). What the write-back copied of the bytes the projection was read back
/// from is what it writes when it copies nothing, byte for byte (roll-lqjj.44).
fn read_back(projection: &Projection, file: &mut TextFile, back: WriteBack) -> Projection {
    let compacts = back.compacts(projection);
    let (encoded, _) = match back {
        WriteBack::Encoded => projection.encode(file.len() as u64),
        _ => projection.encode_with(
            file.len() as u64,
            if compacts {
                Compaction::Compact
            } else {
                Compaction::Keep
            },
        ),
    };
    let (written, _) = projection.encode_copying(
        file.len() as u64,
        if compacts {
            Compaction::Compact
        } else {
            Compaction::Keep
        },
        Copying::Nothing,
    );
    for (part, copied, written) in [
        ("state", &encoded.state, &written.state),
        ("index", &encoded.index, &written.index),
    ] {
        let differs = (copied.iter().zip(written.iter())).position(|(a, b)| a != b);
        assert!(
            copied == written,
            "the {part} part copied differs from the one written, at byte {differs:?} of {}",
            copied.len()
        );
    }
    file.extend_from_slice(&encoded.texts);
    let decoded = Projection::decode(encoded.clone()).unwrap();
    assert!(decoded.state().issues().all(|issue| !issue.texts().here()));
    // each name and each value once in each table, however many write-backs added to it
    let kept = decoded.kept.as_deref().unwrap();
    for table in [&kept.state_table, &kept.index_table] {
        let once = |at: usize| !table.names[..at].contains(&table.names[at]);
        assert!((0..table.names.len()).all(once), "{:?}", table.names);
        let values: Vec<Vec<u8>> = table
            .values
            .iter()
            .map(|v| super::codec::bytes_of(v))
            .collect();
        let once = |at: usize| !values[..at].contains(&values[at]);
        assert!((0..values.len()).all(once), "{:?}", table.values);
    }
    let original = whole(projection, file);
    let read = whole(&decoded, file);
    assert!(same(read.state(), original.state()));
    assert_eq!(
        edges::metadata(read.state()),
        edges::metadata(original.state())
    );
    assert!(known::knows_alike(
        &decoded,
        projection,
        if compacts {
            Compaction::Compact
        } else {
            Compaction::Keep
        }
    ));
    let mut state = decode_state(&encoded.state).unwrap();
    assert!(known::knows_writers_alone(&state, projection));
    assert_eq!(edges::outline(&state), edges::outline(original.state()));
    // as a command that prints some issues whole reads in their texts alone
    let some: Vec<EventId> = state.issues.keys().copied().step_by(2).collect();
    let mut partly = state.clone();
    read_in(&mut partly, file, Some(some.clone()));
    owned::read_alike(&partly, original.state(), &some);
    read_in(&mut state, file, None);
    assert!(same(&state, original.state()));
    assert_eq!(edges::metadata(&state), edges::metadata(original.state()));
    decoded
}

/// Whether `extended` is what projecting the whole journal (`full`) makes, and answers as it does
/// of every event its state names, and of what each machine's next event builds on.
fn equal(extended: &Projection, full: &Projection) -> bool {
    same(extended.state(), full.state())
        && answers_alike(extended, full.state())
        && (MACHINES.iter()).all(|machine| extended.causes_for(machine) == full.causes_for(machine))
        && extended.heads == full.heads
        && extended.ids == full.ids
        && extended.lengths == full.lengths
        && extended.lasts == full.lasts
        && extended.tails == full.tails
        && extended.open == full.open
}

/// Whether `projection` answers as `state`, the same journal's, of every event its state names and
/// its frontier: who wrote each, and whether each head's writer had seen it, what an event applied
/// after all of them asks (roll-lqjj.43).
fn answers_alike(projection: &Projection, state: &State) -> bool {
    let mut named = super::named(projection.state());
    named.extend(projection.heads.iter().copied());
    (named.iter()).all(|id| {
        projection.state().who(*id) == state.who(*id)
            && (projection.heads.iter())
                .all(|head| projection.state().saw(*head, *id) == state.saw(*head, *id))
    })
}

#[test]
fn every_extension_is_replays_answer_or_declined() {
    let tally = differential(0..40, HistoryKind::Valid);
    // A local write builds on the frontier: the projection takes it, a takeover's excepted. A
    // merge it takes when the merged events build on it.
    assert!(
        tally.declined.is_empty(),
        "local writes declined in seeds {:?}",
        tally.declined
    );
    assert!(tally.merged > 50, "{} merges taken in", tally.merged);
    assert_eq!(tally.closed, 0);
    // a projection written back several batches before takes them all in at once (roll-lqjj.36)
    assert!(
        tally.deferred > 500 && tally.longest >= 8,
        "{} extensions by several batches, {} at most",
        tally.deferred,
        tally.longest
    );
    // written back as it was, what every event had seen holds events the state no longer names
    // (roll-lqjj.43)
    assert!(
        tally.uncompacted > 500 && tally.unnamed >= 20,
        "{} write-backs left events the state no longer names, {} at most",
        tally.uncompacted,
        tally.unnamed
    );
    // and as the store writes it back, on both sides of an eighth more
    assert!(
        tally.left > 200 && tally.cut > 200,
        "the store's write-backs: {} left such events, {} compacted",
        tally.left,
        tally.cut
    );
    edges::pinned("plain", &tally.answers);
    tally.drawn.enough();
    // every kind of ownership record, a takeover's reason, a lost claim and an end too late among
    // them, and later events adding to histories a projection read back holds
    assert!(tally.seen.more_than(5), "{:?}", tally.seen);
    assert_eq!(tally.owned, owned::DIFFERENTIAL, "{:#x}", tally.owned);
    tally.marked.enough();
}
