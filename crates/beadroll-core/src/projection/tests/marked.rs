//! What a write-back may copy, and what it must write again (roll-lqjj.44, step 4 of X17): every
//! issue whose record changed since the projection was read back is one the projection marked,
//! every issue it read back keeps its position, and every event it read back keeps its record. A
//! write-back that copied the bytes of an issue it did not mark would copy them stale, and the next
//! command would read them as current; [`check`] finds such an issue at the extension that changed
//! it, on every history the differential tests extend, and [`Exercised`] keeps the generator
//! drawing every path by which an extension changes an issue.

use std::collections::{BTreeMap, HashMap};

use crate::id::EventId;
use crate::journal::{Event, Journal};
use crate::json::Value;
use crate::state::{Issue, TextPlace, Texts};

use super::super::codec::Writer;
use super::super::state_codec::write_issue;
use super::super::{Projection, Record, project};
use super::history;

/// The place a record gives texts written nowhere yet: no texts written have it, so an issue whose
/// texts are to be placed anew always has a record of its own.
const ANEW: TextPlace = TextPlace {
    at: u64::MAX,
    len: u32::MAX,
    sum: [0xff; 16],
};

/// An issue's record as a write-back writes it, alone: with a writer of its own, so that its names
/// and values are written in it, and the record says what the issue is and where its texts are,
/// whatever the issues before it hold.
pub(super) fn record_of(issue: &Issue) -> Vec<u8> {
    let mut w = Writer::default();
    write_issue(&mut w, issue, ANEW);
    w.finish()
}

/// What a projection holds, record by record: each issue's id and record and each event's record,
/// by position, and how many copies were renamed.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Records {
    issues: Vec<(EventId, Vec<u8>)>,
    events: Vec<String>,
    renames: usize,
}

impl Records {
    pub(super) fn of(projection: &Projection) -> Records {
        let state = projection.state();
        let knowledge = &state.knowledge;
        Records {
            issues: (state.issues.values())
                .map(|issue| (issue.uid, record_of(issue)))
                .collect(),
            events: (0..knowledge.events.len())
                .filter_map(|at| knowledge.record_of(at))
                .collect(),
            renames: state.renames.len(),
        }
    }
}

/// What changed of the records `read` in `now`: the positions of the issues whose record differs,
/// the first position that holds another issue or none, and the first event whose record differs or
/// is gone.
#[derive(Debug, Default, PartialEq)]
pub(super) struct Changed {
    pub(super) issues: Vec<usize>,
    pub(super) moved: Option<usize>,
    pub(super) event: Option<usize>,
}

pub(super) fn changed(read: &Records, now: &Records) -> Changed {
    let mut out = Changed::default();
    for (at, (uid, record)) in read.issues.iter().enumerate() {
        match now.issues.get(at) {
            Some((still, now)) if still == uid => {
                if now != record {
                    out.issues.push(at);
                }
            }
            _ => {
                out.moved.get_or_insert(at);
            }
        }
    }
    out.event = (read.events.iter().enumerate())
        .find(|(at, record)| now.events.get(*at) != Some(record))
        .map(|(at, _)| at);
    out
}

/// Whether the issue at position `at` of `projection` is one a write-back writes again rather than
/// copies. Every one, so far: the encode writes every issue.
pub(super) fn marked(_projection: &Projection, _at: usize) -> bool {
    true
}

/// What the extensions the differential tests checked changed: of the issues read back, those a
/// record about them changed, by what the record does, and those no record is about, which a
/// create of a key they waited for bound; the issues created and the copies renamed; the issues
/// whose texts are to be placed anew; the issues read back whose texts were read in and whose record
/// stayed as read; and the events taken in after a read back.
#[derive(Debug, Default)]
pub(in super::super) struct Exercised {
    pub(super) checked: usize,
    pub(super) by: BTreeMap<&'static str, usize>,
    pub(super) bound: usize,
    pub(super) created: usize,
    pub(super) renamed: usize,
    pub(super) placed: usize,
    pub(super) read_in: usize,
    pub(super) appended: usize,
}

impl Exercised {
    /// Adds `other`'s counts.
    pub(super) fn add(&mut self, other: &Exercised) {
        self.checked += other.checked;
        for (what, n) in &other.by {
            *self.by.entry(what).or_default() += n;
        }
        self.bound += other.bound;
        self.created += other.created;
        self.renamed += other.renamed;
        self.placed += other.placed;
        self.read_in += other.read_in;
        self.appended += other.appended;
    }

    /// Whether the clean differential histories drew every path, about half as often as they do.
    pub(super) fn enough(&self) {
        let by = |what: &str| self.by.get(what).copied().unwrap_or(0);
        assert!(
            self.checked > 5_000
                && by("set") > 4_000
                && by("ownership") > 6_000
                && by("delete") > 500
                && by("comment") > 2_500
                && by("edge") > 3_500
                && by("label") > 2_500
                && self.bound > 30
                && self.created > 4_500
                && self.renamed > 2_000
                && self.placed > 7_000
                && self.read_in > 2_000
                && self.appended > 25_000,
            "{self:?}"
        );
    }
}

/// What a record does, as [`Exercised`] counts it.
fn kind(op: &str) -> &'static str {
    match op {
        "set" => "set",
        "claim" | "release" | "close" | "reopen" => "ownership",
        "delete" => "delete",
        "comment" => "comment",
        "dep_add" | "dep_remove" => "edge",
        "label_add" | "label_remove" | "label_set" => "label",
        "create" => "create",
        _ => "other",
    }
}

/// Checks `after`, a projection read back as `read` and extended by `records` from `before`: every
/// issue whose record changed since it was read back is marked, every issue read back is where it
/// was, and every event read back has its record still; an issue changed that no record is about
/// is one that waited for the key a create among them brings (`about`, [`Projection::about`]).
/// Counts what the extension changed.
pub(super) fn check(
    read: &Records,
    before: &Records,
    after: &Projection,
    records: &[Record],
    about: &[EventId],
    tally: &mut Exercised,
) {
    let now = Records::of(after);
    let since = changed(read, &now);
    assert_eq!(since.moved, None, "an issue read back moved");
    assert_eq!(since.event, None, "an event read back changed");
    for at in &since.issues {
        assert!(
            marked(after, *at),
            "issue {} changed and is not marked",
            read.issues[*at].0
        );
    }
    tally.checked += 1;
    let mut ops: HashMap<EventId, Vec<&'static str>> = HashMap::new();
    for record in records {
        let issue = (record.value.get("issue").and_then(Value::as_str)).and_then(EventId::parse);
        let op = record.value.get("op").and_then(Value::as_str);
        if let (Some(issue), Some(op)) = (issue, op) {
            ops.entry(issue).or_default().push(kind(op));
        }
    }
    let issues = after.state().issues.values().collect::<Vec<_>>();
    for at in changed(before, &now).issues {
        let Some((uid, _)) = read.issues.get(at) else {
            continue; // created since it was read back: written whole whatever happens
        };
        match ops.get(uid) {
            Some(kinds) => {
                for kind in kinds {
                    *tally.by.entry(kind).or_default() += 1;
                }
            }
            None => {
                assert!(
                    about.contains(uid),
                    "issue {uid} changed, and nothing is about it"
                );
                tally.bound += 1;
            }
        }
        tally.placed += usize::from(matches!(issues[at].texts, Texts::Here));
    }
    tally.created += now.issues.len() - before.issues.len();
    tally.renamed += usize::from(now.renames > before.renames);
    tally.read_in += (read.issues.iter().zip(&now.issues).zip(&issues))
        .filter(|((was, now), issue)| was == now && matches!(issue.texts, Texts::Read(_)))
        .count();
    tally.appended += now.events.len() - before.events.len();
}

/// Reads in, now and then, the texts of a few issues of `projection` besides those a batch is
/// about, as a command that prints them reads them in, from `file`: so that the tests see issues
/// whose texts were read in and that nothing else changed. Draws from a `coin` of its own, so that
/// the world's draws stay as they were.
pub(super) fn read_more(projection: &mut Projection, file: &[u8], coin: &mut super::Rng) {
    if !coin.chance(30) {
        return;
    }
    let there: Vec<EventId> = (projection.state().issues.values())
        .filter(|issue| matches!(issue.texts, Texts::There(_)))
        .map(|issue| issue.uid)
        .collect();
    let picked = (0..3).filter_map(|_| coin.pick(&there).copied()).collect();
    super::read_in(projection.state_mut(), file, Some(picked));
}

/// What a track's projection holds before an extension, once a few more texts are read in
/// ([`read_more`]), when it was read back as `read`: what [`checked`] compares with.
pub(super) fn ahead(
    projection: &mut Projection,
    file: &[u8],
    coin: &mut super::Rng,
    read: Option<&Records>,
) -> Option<Records> {
    read_more(projection, file, coin);
    read.map(|_| Records::of(projection))
}

/// [`check`], when the projection extended was read back as `read` and held `ahead` before.
pub(super) fn checked(
    read: Option<&Records>,
    ahead: Option<Records>,
    after: &Projection,
    records: &[Record],
    about: &[EventId],
    tally: &mut Exercised,
) {
    if let (Some(read), Some(ahead)) = (read, ahead) {
        check(read, &ahead, after, records, about, tally);
    }
}

/// A projection of the clean history of `seed`, read back.
fn read_back(seed: u64) -> Projection {
    let mut journal = Journal::new();
    for record in history(seed, 30, false) {
        journal.add(
            record.value,
            &record.replica,
            &record.origin,
            record.sequenced,
        );
    }
    let (encoded, _) = project(journal).unwrap().encode(0);
    Projection::decode(&encoded).unwrap()
}

#[test]
fn an_issue_changed_moved_or_an_event_changed_is_seen() {
    let mut projection = read_back(7);
    let read = Records::of(&projection);
    assert_eq!(changed(&read, &read), Changed::default());
    // one field of the third issue
    let uid = *projection.state.issues.get_index(2).unwrap().0;
    let issue = projection.state.issues.get_mut(&uid).unwrap();
    issue.stamps.updated_at = Value::from("2026-10-17T00:00:00Z");
    let now = Records::of(&projection);
    assert_eq!(changed(&read, &now).issues, [2]);
    // its texts to be placed anew
    let mut placed = read_back(7);
    placed.state.issues.get_mut(&uid).unwrap().texts = Texts::Here;
    assert_eq!(changed(&read, &Records::of(&placed)).issues, [2]);
    // the first two issues in each other's places
    let mut moved = read.clone();
    moved.issues.swap(0, 1);
    assert_eq!(changed(&read, &moved).moved, Some(0));
    // the fifth event's machine
    let knowledge = &mut projection.state.knowledge;
    let (id, event) = knowledge.events.get_index_mut(4).unwrap();
    let deps = event.deps().into();
    *event = Event::applied(*id, deps, "elsewhere".into(), event.shared_actor().cloned());
    assert_eq!(changed(&read, &Records::of(&projection)).event, Some(4));
    // an event gone
    let mut fewer = read.clone();
    fewer.events.pop();
    assert_eq!(changed(&read, &fewer).event, Some(read.events.len() - 1));
}

#[test]
fn reading_texts_in_changes_no_record() {
    let mut journal = Journal::new();
    for record in history(7, 30, false) {
        journal.add(
            record.value,
            &record.replica,
            &record.origin,
            record.sequenced,
        );
    }
    let (encoded, _) = project(journal).unwrap().encode(0);
    let mut projection = Projection::decode(&encoded).unwrap();
    let read = Records::of(&projection);
    super::read_in(projection.state_mut(), &encoded.texts, None);
    let issues = projection.state().issues.values();
    assert!(issues.clone().count() > 20);
    assert!(
        issues
            .into_iter()
            .all(|issue| matches!(issue.texts, Texts::Read(_)))
    );
    assert_eq!(
        changed(&read, &Records::of(&projection)),
        Changed::default()
    );
}
