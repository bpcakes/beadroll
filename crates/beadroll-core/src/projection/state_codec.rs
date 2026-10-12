//! A state's issues, and what replay keeps beside them, as bytes ([`super::codec`]). What replay
//! derives once it has applied every event (each issue's labels, the parent-child index) is not
//! written: it is derived again as the state is read.

use crate::event::Field;
use crate::json::{Int, Value};
use crate::replay::finish;
use crate::state::{
    Claim, Comment, EndedClaim, IdMap, IdSet, Integrity, Issue, Knowledge, LostClaim, Origin,
    Provisional, Rename, Stale, State, VecMap,
};

use super::codec::{Reader, Unreadable, Writer};

/// How a claim can end, as [`EndedClaim::how`] names it.
const ENDINGS: [&str; 5] = ["takeover", "release", "close", "reopen", "delete"];

pub(crate) fn write_state(w: &mut Writer, st: &State) {
    w.optional_value(st.prefix.as_ref());
    w.optional_value(st.name.as_ref());
    w.count(st.issues.len());
    for issue in st.issues.values() {
        write_issue(w, issue);
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
    w.ids(&st.pending);
    w.number(st.orphans);
    w.number(st.comment_count);
    w.count(st.last_by_thread.len());
    for ((replica, thread), id) in &st.last_by_thread {
        w.text(replica);
        w.optional_text(thread.as_deref());
        w.id(*id);
    }
    w.ids(&st.awaiting);
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
    st.awaiting = r.ids()?;
    finish(&mut st);
    Ok(st)
}

fn write_issue(w: &mut Writer, issue: &Issue) {
    w.id(issue.uid);
    w.text(&issue.key);
    for field in &issue.fields {
        w.optional_value(field.as_ref());
    }
    w.count(issue.reg.len());
    for (field, tags) in &issue.reg {
        w.count(field.index());
        write_tags(w, tags);
    }
    w.count(issue.label_tags.len());
    for (label, tags) in &issue.label_tags {
        w.text(label);
        w.ids(tags.iter().as_slice());
    }
    w.count(issue.dep_tags.len());
    for ((to, kind), tags) in &issue.dep_tags {
        w.text(to);
        w.text(kind);
        write_tags(w, tags);
    }
    w.count(issue.comments.len());
    for comment in &issue.comments {
        match comment {
            Comment::Native {
                uid,
                seq,
                author,
                text,
                where_,
            } => {
                w.byte(0);
                w.id(*uid);
                w.number(*seq);
                w.name(author);
                w.text(text);
                w.optional_shared(where_.as_ref());
            }
            Comment::Imported(members) => {
                w.byte(1);
                w.object(members);
            }
        }
    }
    w.flag(issue.extra.is_some());
    if let Some(extra) = &issue.extra {
        w.object(extra);
    }
    write_values(w, &issue.dangling);
    w.count(issue.retracted.len());
    for ((to, kind), removals) in &issue.retracted {
        w.text(to);
        w.optional_text(kind.as_deref());
        w.ids(removals.iter().as_slice());
    }
    write_ownership(w, issue);
    write_values(w, &issue.unconfirmed);
    w.name(&issue.origin.machine);
    w.optional_shared(issue.origin.actor.as_ref());
    w.optional_shared(issue.origin.place.as_ref());
    w.optional_shared(issue.where_.as_ref());
    w.optional_shared(issue.closed_on.as_ref());
}

fn read_issue(r: &mut Reader<'_>) -> Result<Issue, Unreadable> {
    let uid = r.id()?;
    let key = r.text()?;
    let mut fields: [Option<Value>; 16] = Default::default();
    for field in &mut fields {
        *field = r.optional_value()?;
    }
    let mut reg = Vec::new();
    for _ in 0..r.count()? {
        let field = Field::ALL.get(r.index()?).copied();
        reg.push((field.ok_or(Unreadable("a field"))?, read_tags(r)?));
    }
    let mut label_tags = Vec::new();
    for _ in 0..r.count()? {
        label_tags.push((r.text()?, read_ids(r)?));
    }
    let mut dep_tags = Vec::new();
    for _ in 0..r.count()? {
        let edge = (r.text()?, r.text()?);
        dep_tags.push((edge, read_tags(r)?));
    }
    let mut comments = Vec::new();
    for _ in 0..r.count()? {
        comments.push(match r.byte()? {
            0 => Comment::Native {
                uid: r.id()?,
                seq: r.number()?,
                author: r.name()?,
                text: r.text()?,
                where_: r.optional_shared()?,
            },
            1 => Comment::Imported(r.object()?),
            _ => return Err(Unreadable("a comment")),
        });
    }
    let extra = r.flag()?.then(|| r.object()).transpose()?.map(Box::new);
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
    Ok(Issue {
        uid,
        key,
        fields,
        reg: VecMap::from_distinct(reg),
        label_tags: VecMap::from_distinct(label_tags),
        dep_tags: VecMap::from_distinct(dep_tags),
        labels: std::collections::BTreeSet::new(),
        comments,
        extra,
        dangling,
        retracted,
        claim: held.claim,
        claim_generation: held.claim_generation,
        claim_history: held.claim_history,
        provisional_claims: held.provisional_claims,
        lost_claims: held.lost_claims,
        stale: held.stale,
        unconfirmed,
        origin,
        where_: r.optional_shared()?,
        closed_on: r.optional_shared()?,
    })
}

/// Who held an issue, as [`Issue`] records it.
struct Ownership {
    claim: Option<Box<Claim>>,
    claim_generation: Int,
    claim_history: Vec<EndedClaim>,
    provisional_claims: Vec<Provisional>,
    lost_claims: Vec<LostClaim>,
    stale: Vec<Stale>,
}

fn write_ownership(w: &mut Writer, issue: &Issue) {
    w.flag(issue.claim.is_some());
    if let Some(claim) = &issue.claim {
        write_claim(w, claim);
    }
    w.value(&Value::Int(issue.claim_generation.clone()));
    w.count(issue.claim_history.len());
    for ended in &issue.claim_history {
        write_claim(w, &ended.claim);
        w.id(ended.ending);
        w.name(&ended.ended_by);
        let how = ENDINGS.iter().position(|how| *how == ended.how);
        w.count(how.unwrap_or(ENDINGS.len()));
        w.optional_text(ended.reason.as_deref());
    }
    w.count(issue.provisional_claims.len());
    for offline in &issue.provisional_claims {
        w.id(offline.id);
        w.name(&offline.owner);
        w.optional_shared(offline.where_.as_ref());
    }
    w.count(issue.lost_claims.len());
    for lost in &issue.lost_claims {
        w.name(&lost.by);
        w.name(&lost.held_by);
        w.id(lost.event);
    }
    w.count(issue.stale.len());
    for stale in &issue.stale {
        w.text(&stale.op);
        w.name(&stale.by);
        w.text(&stale.at);
        w.optional_text(stale.claim.as_deref());
    }
}

fn read_ownership(r: &mut Reader<'_>) -> Result<Ownership, Unreadable> {
    let claim = r.flag()?.then(|| read_claim(r)).transpose()?.map(Box::new);
    let claim_generation = read_int(r)?;
    let mut claim_history = Vec::new();
    for _ in 0..r.count()? {
        claim_history.push(EndedClaim {
            claim: read_claim(r)?,
            ending: r.id()?,
            ended_by: r.name()?,
            how: ENDINGS
                .get(r.index()?)
                .copied()
                .ok_or(Unreadable("an ending"))?,
            reason: r.optional_text()?,
        });
    }
    let mut provisional_claims = Vec::new();
    for _ in 0..r.count()? {
        provisional_claims.push(Provisional {
            id: r.id()?,
            owner: r.name()?,
            where_: r.optional_shared()?,
        });
    }
    let mut lost_claims = Vec::new();
    for _ in 0..r.count()? {
        lost_claims.push(LostClaim {
            by: r.name()?,
            held_by: r.name()?,
            event: r.id()?,
        });
    }
    let mut stale = Vec::new();
    for _ in 0..r.count()? {
        stale.push(Stale {
            op: r.text()?,
            by: r.name()?,
            at: r.text()?,
            claim: r.optional_text()?,
        });
    }
    Ok(Ownership {
        claim,
        claim_generation,
        claim_history,
        provisional_claims,
        lost_claims,
        stale,
    })
}

fn write_claim(w: &mut Writer, claim: &Claim) {
    w.id(claim.id);
    w.name(&claim.owner);
    w.name(&claim.machine);
    w.optional_name(claim.agent.as_ref());
    w.optional_name(claim.thread.as_ref());
    w.value(&Value::Int(claim.generation.clone()));
    w.name(&claim.mode);
    w.optional_shared(claim.where_.as_ref());
}

fn read_claim(r: &mut Reader<'_>) -> Result<Claim, Unreadable> {
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

fn write_tags(w: &mut Writer, tags: &IdMap<Value>) {
    w.count(tags.len());
    for (id, value) in tags {
        w.id(*id);
        w.value(value);
    }
}

fn read_tags(r: &mut Reader<'_>) -> Result<IdMap<Value>, Unreadable> {
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
