//! A state's issues, and what replay keeps beside them, as bytes ([`super::codec`]). What replay
//! derives once it has applied every event (each issue's labels, the parent-child index) is not
//! written: it is derived again as the state is read, and so is where each imported dependency
//! that waits for a key is (roll-lqjj.18), from the issues' lists.
//!
//! An issue's texts (its long fields, the values their registers hold, its comments, its extras,
//! the metadata of its edges' additions and its ownership history) are written apart, each issue's
//! as bytes of their own ([`super::text_codec`]), which a projection keeps in its text file
//! (roll-lqjj.6, roll-lqjj.39, roll-lqjj.40); the issue is written with where they are and what a
//! list, a later event and a command's decisions need of them unread ([`Stored`]), with its edges
//! typed and the events that added each (roll-lqjj.39), and with its current claim, its generation
//! and its offline claims, which every ownership decision reads. An issue read back has its texts
//! there until they are read in.

use std::collections::HashMap;

use crate::event::Field;
use crate::id::EventId;
use crate::json::{Int, Value};
use crate::replay::finish;
use crate::state::{
    Awaiting, Claim, DepTags, Edge, Ended, IdMap, IdSet, Integrity, Issue, Knowledge, Meta, Origin,
    Provisional, Rename, Stamps, State, Stored, Target, TextPlace, Texts, VecMap, is_text,
};

use super::codec::{Reader, Unreadable, Writer};
use super::text_codec::summary;

/// The place of texts written nowhere: no texts' sum, so never read as an issue's.
const NOWHERE: TextPlace = TextPlace {
    at: 0,
    len: 0,
    sum: [0; 16],
};

/// How a claim can end, as [`crate::state::EndedClaim::how`] names it.
const ENDINGS: [&str; 5] = ["takeover", "release", "close", "reopen", "delete"];

/// The state, each issue with where its texts are: where they were read from or are, or for texts
/// written nowhere yet, the place `given` gives them.
pub(crate) fn write_state(w: &mut Writer, st: &State, given: &HashMap<EventId, TextPlace>) {
    w.optional_value(st.prefix.as_ref());
    w.optional_value(st.name.as_ref());
    w.count(st.issues.len());
    for issue in st.issues.values() {
        write_issue(w, issue, given.get(&issue.uid).copied().unwrap_or(NOWHERE));
    }
    w.count(st.by_key.len());
    for (key, uid) in &st.by_key {
        w.text(key);
        w.id(*uid);
    }
    w.count(st.renames.len());
    for rename in &st.renames {
        w.text(&rename.from);
        w.text(&rename.to);
        w.text(&rename.replica);
        w.id(rename.uid);
    }
    w.count(st.integrity.len());
    for entry in &st.integrity {
        w.optional_text(entry.event.as_deref());
        w.optional_text(entry.issue.as_deref());
        w.text(&entry.problem);
        w.optional_text(entry.origin.as_deref());
    }
    w.ids(st.pending.iter());
    w.number(st.orphans);
    w.number(st.comment_count);
    w.count(st.last_by_thread.len());
    for ((replica, thread), id) in &st.last_by_thread {
        w.text(replica);
        w.optional_text(thread.as_deref());
        w.id(*id);
    }
}

/// A state, what every event had seen left out: the caller gives it the knowledge it has.
pub(crate) fn read_state(r: &mut Reader<'_>) -> Result<State, Unreadable> {
    let mut st = State::new(Knowledge::default());
    st.prefix = r.optional_value()?;
    st.name = r.optional_value()?;
    for _ in 0..r.count()? {
        let issue = read_issue(r)?;
        st.issues.insert(issue.uid, Box::new(issue));
    }
    for _ in 0..r.count()? {
        let key = r.text()?;
        st.by_key.insert(key, r.id()?);
    }
    for _ in 0..r.count()? {
        st.renames.push(Rename {
            from: r.text()?,
            to: r.text()?,
            replica: r.text()?,
            uid: r.id()?,
        });
    }
    for _ in 0..r.count()? {
        st.integrity.push(Integrity {
            event: r.optional_text()?,
            issue: r.optional_text()?,
            problem: r.text()?,
            origin: r.optional_text()?,
        });
    }
    st.pending = r.ids()?;
    st.orphans = r.number()?;
    st.comment_count = r.number()?;
    for _ in 0..r.count()? {
        let thread = (r.text()?, r.optional_text()?);
        st.last_by_thread.insert(thread, r.id()?);
    }
    st.awaiting = Awaiting::of(st.issues.values().map(|issue| &**issue));
    finish(&mut st);
    st.issues.hold_as_read();
    Ok(st)
}

/// An issue without its texts, and where they are (`place`, for texts written nowhere yet).
pub(super) fn write_issue(w: &mut Writer, issue: &Issue, place: TextPlace) {
    w.id(issue.uid);
    w.text(&issue.key);
    let stamps = &issue.stamps;
    w.value(&stamps.created_at);
    w.value(&stamps.created_by);
    w.value(&stamps.updated_at);
    w.flag(stamps.own_closed_at);
    w.flag(stamps.own_close_reason);
    let registers: Vec<_> = (issue.reg.iter())
        .filter(|(field, _)| !is_text(**field))
        .collect();
    w.count(registers.len());
    for (field, tags) in registers {
        w.count(field.index());
        write_tags(w, tags);
    }
    w.count(issue.label_tags.len());
    for (label, tags) in &issue.label_tags {
        w.text(label);
        w.ids(tags.iter());
    }
    write_edges(w, &issue.dep_tags);
    write_values(w, &issue.dangling);
    w.count(issue.retracted.len());
    for ((to, kind), removals) in &issue.retracted {
        w.text(to);
        w.optional_text(kind.as_deref());
        w.ids(removals.iter());
    }
    write_ownership(w, issue);
    write_values(w, &issue.unconfirmed);
    w.name(&issue.origin.machine);
    w.optional_shared(issue.origin.actor.as_ref());
    w.optional_shared(issue.origin.place.as_ref());
    w.optional_shared(issue.where_.as_ref());
    w.optional_shared(issue.closed_on.as_ref());
    let stored = match &issue.texts {
        Texts::There(stored) => (**stored).clone(),
        Texts::Read(place) => summary(issue, *place),
        Texts::Here => summary(issue, place),
    };
    w.number(stored.place.at);
    w.number(u64::from(stored.place.len));
    w.sixteen(&stored.place.sum);
    w.count(stored.tags.len());
    for (field, ids) in &stored.tags {
        w.count(field.index());
        w.ids(ids.iter());
    }
    w.count(stored.disputed.len());
    for field in &stored.disputed {
        w.count(field.index());
    }
    w.flag(stored.ended.is_some());
    if let Some(ended) = &stored.ended {
        w.name(&ended.by);
        write_how(w, ended.how);
    }
}

fn read_issue(r: &mut Reader<'_>) -> Result<Issue, Unreadable> {
    let uid = r.id()?;
    let key = r.text()?;
    let stamps = Stamps {
        created_at: r.value()?,
        created_by: r.value()?,
        updated_at: r.value()?,
        own_closed_at: r.flag()?,
        own_close_reason: r.flag()?,
    };
    let mut reg = Vec::new();
    for _ in 0..r.count()? {
        let field = Field::ALL.get(r.index()?).copied();
        reg.push((field.ok_or(Unreadable("a field"))?, read_tags(r)?));
    }
    let mut label_tags = Vec::new();
    for _ in 0..r.count()? {
        label_tags.push((r.text()?, read_ids(r)?));
    }
    let dep_tags = read_edges(r)?;
    let dangling = read_values(r)?;
    let mut retracted = std::collections::BTreeMap::new();
    for _ in 0..r.count()? {
        let removed = (r.text()?, r.optional_text()?);
        retracted.insert(removed, read_ids(r)?);
    }
    let held = read_ownership(r)?;
    let unconfirmed = read_values(r)?;
    let origin = Origin {
        machine: r.name()?,
        actor: r.optional_shared()?,
        place: r.optional_shared()?,
    };
    let where_ = r.optional_shared()?;
    let closed_on = r.optional_shared()?;
    let place = TextPlace {
        at: r.number()?,
        len: r.small()?,
        sum: r.sixteen()?,
    };
    let mut tags = Vec::new();
    for _ in 0..r.count()? {
        let field = Field::ALL.get(r.index()?).copied();
        tags.push((field.ok_or(Unreadable("a long field"))?, r.ids()?));
    }
    let mut disputed = Vec::new();
    for _ in 0..r.count()? {
        let field = Field::ALL.get(r.index()?).copied();
        disputed.push(field.ok_or(Unreadable("a long field"))?);
    }
    let ended = r.flag()?.then(|| read_ended(r)).transpose()?;
    Ok(Issue {
        uid,
        key,
        stamps,
        reg: VecMap::from_distinct(reg),
        label_tags: VecMap::from_distinct(label_tags),
        dep_tags,
        labels: std::collections::BTreeSet::new(),
        comments: Vec::new(),
        extra: None,
        dangling,
        retracted,
        claim: held.claim,
        claim_generation: held.claim_generation,
        claim_history: Vec::new(),
        provisional_claims: held.provisional_claims,
        lost_claims: Vec::new(),
        stale: Vec::new(),
        unconfirmed,
        origin,
        where_,
        closed_on,
        texts: Texts::There(Box::new(Stored {
            place,
            tags,
            disputed,
            ended,
        })),
    })
}

/// An issue's edges (roll-lqjj.39): each one's target, an id as its 16 bytes or text, its kind
/// through the name table, so that each kind is written once, and the events that added it. Their
/// metadata is with the issue's texts ([`super::text_codec`]).
fn write_edges(w: &mut Writer, edges: &DepTags) {
    w.count(edges.len());
    for (edge, tags) in edges {
        match &edge.to {
            Target::Id(id) => {
                w.byte(0);
                w.id(*id);
            }
            Target::Text(text) => {
                w.byte(1);
                w.text(text);
            }
        }
        w.name(&edge.kind);
        w.ids(tags.into_iter().map(|(id, _)| id));
    }
}

/// The edges [`write_edges`] wrote, each addition's metadata not read: an issue read back has its
/// texts there. Marked `#[inline]`, though it stays a call of its own: without the mark, the
/// ownership-heavy history at 200,000 events, which has no edges, decoded 0.8 and 1.45 ms slower
/// (median of 33) in two alternated sets of 8 and 10 rounds, and alike in a review's set, the code
/// read per issue being the same; what the mark changes is how the code around it is compiled,
/// which another change may undo.
#[inline]
fn read_edges(r: &mut Reader<'_>) -> Result<DepTags, Unreadable> {
    let n = r.count()?;
    let mut edges = Vec::with_capacity(n);
    for _ in 0..n {
        let to = match r.byte()? {
            0 => Target::Id(r.id()?),
            1 => Target::text(r.text()?).ok_or(Unreadable("a target"))?,
            _ => return Err(Unreadable("a target")),
        };
        let edge = Edge {
            to,
            kind: r.name()?,
        };
        let tags = r.ids()?.into_iter().map(|id| (id, Meta::default()));
        let tags = IdMap::from_ascending(tags.collect()).ok_or(Unreadable("an edge's tags"))?;
        edges.push((edge, tags));
    }
    Ok(VecMap::from_distinct(edges))
}

/// Who holds an issue and how many times it was claimed, as [`Issue`] records it: what every
/// ownership decision reads, which its ownership history, kept with its texts, is not.
struct Ownership {
    claim: Option<Box<Claim>>,
    claim_generation: Int,
    provisional_claims: Vec<Provisional>,
}

fn write_ownership(w: &mut Writer, issue: &Issue) {
    w.flag(issue.claim.is_some());
    if let Some(claim) = &issue.claim {
        write_claim(w, claim);
    }
    w.value(&Value::Int(issue.claim_generation.clone()));
    w.count(issue.provisional_claims.len());
    for offline in &issue.provisional_claims {
        w.id(offline.id);
        w.name(&offline.owner);
        w.optional_shared(offline.where_.as_ref());
    }
}

fn read_ownership(r: &mut Reader<'_>) -> Result<Ownership, Unreadable> {
    let claim = r.flag()?.then(|| read_claim(r)).transpose()?.map(Box::new);
    let claim_generation = read_int(r)?;
    let mut provisional_claims = Vec::new();
    for _ in 0..r.count()? {
        provisional_claims.push(Provisional {
            id: r.id()?,
            owner: r.name()?,
            where_: r.optional_shared()?,
        });
    }
    Ok(Ownership {
        claim,
        claim_generation,
        provisional_claims,
    })
}

/// How a claim ended, as one of [`ENDINGS`].
pub(super) fn write_how(w: &mut Writer, how: &str) {
    let at = ENDINGS.iter().position(|known| *known == how);
    w.count(at.unwrap_or(ENDINGS.len()));
}

pub(super) fn read_how(r: &mut Reader<'_>) -> Result<&'static str, Unreadable> {
    (ENDINGS.get(r.index()?).copied()).ok_or(Unreadable("an ending"))
}

fn read_ended(r: &mut Reader<'_>) -> Result<Ended, Unreadable> {
    Ok(Ended {
        by: r.name()?,
        how: read_how(r)?,
    })
}

pub(super) fn write_claim(w: &mut Writer, claim: &Claim) {
    w.id(claim.id);
    w.name(&claim.owner);
    w.name(&claim.machine);
    w.optional_name(claim.agent.as_ref());
    w.optional_name(claim.thread.as_ref());
    w.value(&Value::Int(claim.generation.clone()));
    w.name(&claim.mode);
    w.optional_shared(claim.where_.as_ref());
}

pub(super) fn read_claim(r: &mut Reader<'_>) -> Result<Claim, Unreadable> {
    Ok(Claim {
        id: r.id()?,
        owner: r.name()?,
        machine: r.name()?,
        agent: r.optional_name()?,
        thread: r.optional_name()?,
        generation: read_int(r)?,
        mode: r.name()?,
        where_: r.optional_shared()?,
    })
}

fn read_int(r: &mut Reader<'_>) -> Result<Int, Unreadable> {
    match r.value()? {
        Value::Int(n) => Ok(n),
        _ => Err(Unreadable("an integer")),
    }
}

pub(super) fn write_tags(w: &mut Writer, tags: &IdMap<Value>) {
    w.count(tags.len());
    for (id, value) in tags {
        w.id(*id);
        w.value(value);
    }
}

pub(super) fn read_tags(r: &mut Reader<'_>) -> Result<IdMap<Value>, Unreadable> {
    let n = r.count()?;
    let mut tags = Vec::with_capacity(n);
    for _ in 0..n {
        tags.push((r.id()?, r.value()?));
    }
    IdMap::from_ascending(tags).ok_or(Unreadable("tags"))
}

fn read_ids(r: &mut Reader<'_>) -> Result<IdSet, Unreadable> {
    IdSet::from_ascending(r.ids()?).ok_or(Unreadable("ids"))
}

fn write_values(w: &mut Writer, values: &[Value]) {
    w.count(values.len());
    for value in values {
        w.value(value);
    }
}

fn read_values(r: &mut Reader<'_>) -> Result<Vec<Value>, Unreadable> {
    let n = r.count()?;
    (0..n).map(|_| r.value()).collect()
}
