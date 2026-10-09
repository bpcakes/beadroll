//! One event applied to the state (Python's `apply_event`, `write_register`, `write_status`,
//! `materialise_dangling` and `end_claim`), branch for branch.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use indexmap::IndexMap;

use crate::event::{DONE, Field, REGISTERS, int_value};
use crate::id::EventId;
use crate::journal::Event;
use crate::json::{Int, Object, Value};
use crate::state::{
    Claim, Comment, EndedClaim, Issue, KeySpace, Knowledge, LostClaim, Provisional, Rename, Stale,
    State,
};
use crate::text::str_of;
use crate::time::event_time;

/// Applies one event. `Ok(true)` when it touched an existing issue.
pub(super) fn apply_event(
    st: &mut State,
    k: &Knowledge,
    event: &Event,
    who: &str,
    when: &str,
    superseded: &HashSet<EventId>,
) -> Result<bool, KeySpace> {
    let (op, data) = (event.op(), event.data());
    if op == "init" {
        if st.prefix.is_none() {
            st.prefix = data.get("prefix").filter(|v| **v != Value::Null).cloned();
            st.name = data.get("name").filter(|v| **v != Value::Null).cloned();
        }
        return Ok(false);
    }
    if op == "create" {
        create(st, k, event, who, when)?;
        return Ok(false);
    }
    let Some(uid) = event
        .issue()
        .and_then(EventId::parse)
        .filter(|uid| st.issues.contains_key(uid))
    else {
        st.orphans += 1;
        return Ok(false);
    };
    let Some(issue) = st.issues.get_mut(&uid) else {
        return Ok(false);
    };
    let c = Ctx {
        k,
        event,
        eid: event.id(),
        uid,
        op,
        data,
        who,
        when,
        place: event.get("where").filter(|place| place.truthy()).cloned(),
        claim: issue.claim.clone(),
        superseded,
    };
    let applied = match op {
        "set" => set(&c, issue),
        "claim" => claim(&c, issue),
        "release" => release(&c, issue),
        "close" | "delete" => close(&c, issue),
        "reopen" => reopen(&c, issue),
        "comment" => {
            st.comment_count += 1;
            comment(&c, issue, st.comment_count)
        }
        "dep_add" => dep_add(&c, issue),
        "dep_remove" => dep_remove(&c, issue, &st.by_key),
        "label_add" | "label_remove" | "label_set" => label(&c, issue),
        // Anything else that reaches here applies nothing, but touches the issue: a placeholder
        // whose position broke its journal's chain is not inert (Python's replay).
        _ => true,
    };
    Ok(applied)
}

/// What every operation reads: the event, its writer and time, and the issue's claim before it.
struct Ctx<'a> {
    k: &'a Knowledge,
    event: &'a Event,
    eid: EventId,
    uid: EventId,
    op: &'a str,
    data: &'a Object,
    who: &'a str,
    when: &'a str,
    /// The event's `where`, when it names a place.
    place: Option<Value>,
    claim: Option<Claim>,
    superseded: &'a HashSet<EventId>,
}

impl Ctx<'_> {
    /// A text member of the payload.
    fn text(&self, key: &str) -> Option<&str> {
        self.data.get(key).and_then(Value::as_str)
    }

    /// Records an end of a claim that was no longer current; it applies nothing.
    fn stale(&self, issue: &mut Issue, named: Option<&str>) -> bool {
        issue.stale.push(Stale {
            op: self.op.to_owned(),
            by: self.who.to_owned(),
            at: self.when.to_owned(),
            claim: named.map(str::to_owned),
        });
        false
    }

    fn write_status(
        &self,
        issue: &mut Issue,
        status: &str,
        reason: Option<&str>,
        supersede: &[EventId],
    ) {
        write_status(
            self.k, issue, self.eid, status, self.when, reason, supersede,
        );
    }

    fn write(&self, issue: &mut Issue, field: Field, value: Value, supersede: &[EventId]) {
        write_register(self.k, issue, field, self.eid, value, supersede);
    }
}

fn set(c: &Ctx<'_>, issue: &mut Issue) -> bool {
    for (name, value) in c
        .data
        .get("fields")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        match Field::from_name(name) {
            Some(Field::Status) => {
                c.write_status(issue, value.as_str().unwrap_or_default(), None, &[])
            }
            Some(field) => c.write(issue, field, value.clone(), &[]),
            None => {}
        }
    }
    true
}

fn claim(c: &Ctx<'_>, issue: &mut Issue) -> bool {
    let who = c.who;
    if c.text("mode") == Some("offline") {
        // Provisional: never displaces a confirmed owner.
        if !c.claim.as_ref().is_some_and(|held| held.owner == who) {
            issue.provisional_claims.retain(|p| p.owner != who);
            issue.provisional_claims.push(Provisional {
                id: c.eid,
                owner: who.to_owned(),
                since: c.when.to_owned(),
                where_: c.place.clone(),
            });
            if c.claim.is_none() {
                c.write_status(issue, "in_progress", None, &[]);
            }
        }
    } else if c.claim.is_none() || takes_over(&c.claim, c.data) {
        end_claim(issue, "takeover", who, c.when, c.text("reason"));
        let next = issue.claim_generation.plus_one();
        let given = c
            .data
            .get("generation")
            .filter(|g| g.truthy())
            .and_then(int_value);
        issue.claim_generation = given.filter(|given| *given > next).unwrap_or(next);
        issue.claim = Some(Claim {
            id: c.eid,
            owner: who.to_owned(),
            machine: c.event.replica().to_owned(),
            agent: actor_text(c.event, "agent"),
            thread: actor_text(c.event, "thread"),
            generation: issue.claim_generation.clone(),
            since: c.when.to_owned(),
            mode: c
                .text("mode")
                .filter(|m| !m.is_empty())
                .unwrap_or("online")
                .to_owned(),
            where_: c.place.clone(),
        });
        issue.provisional_claims.retain(|p| p.owner != who);
        c.write_status(issue, "in_progress", None, &[]);
        c.write(issue, Field::Assignee, Value::from(who), &[]);
    } else if let Some(held) = c.claim.as_ref().filter(|held| held.owner != who) {
        issue.lost_claims.push(LostClaim {
            by: who.to_owned(),
            at: c.when.to_owned(),
            held_by: held.owner.clone(),
            event: c.eid,
        });
        return false;
    }
    true
}

fn release(c: &Ctx<'_>, issue: &mut Issue) -> bool {
    let named = c.text("claim");
    let named_id = named.and_then(EventId::parse);
    if named_id.is_some_and(|named| c.superseded.contains(&named)) {
        return c.stale(issue, named);
    }
    if let Some(held) = c.claim.as_ref().filter(|held| Some(held.id) == named_id) {
        end_claim(issue, "release", c.who, c.when, c.text("reason"));
        c.write_status(issue, "open", None, &[held.id]);
        c.write(issue, Field::Assignee, Value::Null, &[held.id]);
    } else if issue.provisional_claims.iter().any(|p| p.owner == c.who) {
        issue.provisional_claims.retain(|p| p.owner != c.who);
        if c.claim.is_none() && issue.provisional_claims.is_empty() {
            c.write_status(issue, "open", None, &[]);
        }
    } else {
        return c.stale(issue, named); // a delayed release from an owner that has since been replaced
    }
    true
}

/// `close` and `delete`: both may name the claim they end.
fn close(c: &Ctx<'_>, issue: &mut Issue) -> bool {
    let named = c.text("claim").filter(|named| !named.is_empty());
    let named_id = named.and_then(EventId::parse);
    let current = c
        .claim
        .as_ref()
        .is_some_and(|held| Some(held.id) == named_id);
    if named.is_some() && (named_id.is_some_and(|named| c.superseded.contains(&named)) || !current)
    {
        return c.stale(issue, named); // an owner replaced by a takeover
    }
    let supersede: Vec<EventId> = named_id.into_iter().collect();
    let reason = c.text("reason");
    // A close that names its claim knows about it, even when the claim's bytes arrived later.
    let closing = c.op == "close";
    c.write_status(
        issue,
        if closing { "closed" } else { "tombstone" },
        reason,
        &supersede,
    );
    if !closing {
        issue.extra.insert("deleted_at".into(), Value::from(c.when));
        issue.extra.insert("deleted_by".into(), Value::from(c.who));
        issue.extra.insert(
            "delete_reason".into(),
            Value::from(reason.unwrap_or_default()),
        );
    }
    if c.claim
        .as_ref()
        .is_some_and(|held| named.is_some() || c.k.saw(c.eid, held.id))
    {
        if closing && named.is_some() {
            let assignee = issue.get(Field::Assignee).cloned().unwrap_or(Value::Null);
            c.write(issue, Field::Assignee, assignee, &supersede);
        }
        end_claim(
            issue,
            if closing { "close" } else { "delete" },
            c.who,
            c.when,
            None,
        );
    }
    if closing {
        issue.provisional_claims.retain(|p| !c.k.saw(c.eid, p.id));
        if let Some(place) = &c.place {
            issue.closed_on = Some(place.clone());
        }
    }
    true
}

fn reopen(c: &Ctx<'_>, issue: &mut Issue) -> bool {
    c.write_status(issue, "open", None, &[]);
    issue.closed_on = None;
    if c.claim.as_ref().is_some_and(|held| c.k.saw(c.eid, held.id)) {
        end_claim(issue, "reopen", c.who, c.when, None);
    }
    true
}

fn comment(c: &Ctx<'_>, issue: &mut Issue, seq: u64) -> bool {
    let author = c
        .data
        .get("author")
        .filter(|a| a.truthy())
        .map_or_else(|| c.who.to_owned(), str_of);
    issue.comments.push(Comment::Native {
        uid: c.eid,
        seq,
        author,
        text: c.text("text").unwrap_or_default().to_owned(),
        created_at: c.when.to_owned(),
        where_: c.place.clone(),
    });
    true
}

fn dep_add(c: &Ctx<'_>, issue: &mut Issue) -> bool {
    let to = c.text("to").unwrap_or_default().to_owned();
    let kind = c
        .text("type")
        .filter(|kind| !kind.is_empty())
        .unwrap_or("blocks")
        .to_owned();
    let mut meta = Object::new();
    meta.insert("created_at".into(), Value::from(c.when));
    meta.insert("created_by".into(), Value::from(c.who));
    for (name, value) in c
        .data
        .get("meta")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        meta.insert(name.clone(), value.clone());
    }
    issue
        .dep_tags
        .entry((to, kind))
        .or_default()
        .insert(c.eid, Value::Object(meta));
    true
}

/// Removes only the additions its writer had seen.
fn dep_remove(c: &Ctx<'_>, issue: &mut Issue, by_key: &IndexMap<String, EventId>) -> bool {
    let wanted = c.text("type");
    let matches = |kind: &str| wanted.is_none_or(|wanted| wanted == kind);
    if let Some(key) = c.text("key").filter(|key| !key.is_empty()) {
        // An imported dependency on a key that was not an issue for its writer.
        issue.dangling.retain(|dep| {
            let depends_on = dep.get("depends_on_id").and_then(Value::as_str);
            !(depends_on == Some(key) && matches(&dangling_type(dep)))
        });
        // A concurrent create of the key that sorted first bound it already.
        if let Some(bound) = by_key.get(key).filter(|_| c.k.saw(c.eid, c.uid)) {
            let bound = bound.to_string();
            for ((to, kind), tags) in &mut issue.dep_tags {
                if *to == bound && matches(kind) {
                    tags.remove(&c.uid); // the import's own tag only: an explicit `dep add` keeps its edge
                }
            }
        }
    } else {
        let to = c.text("to").unwrap_or_default();
        for ((edge_to, kind), tags) in &mut issue.dep_tags {
            if edge_to == to && matches(kind) {
                tags.retain(|tag, _| !c.k.saw(c.eid, *tag));
            }
        }
        let removed = (to.to_owned(), wanted.map(str::to_owned));
        issue.retracted.entry(removed).or_default().insert(c.eid);
    }
    true
}

fn label(c: &Ctx<'_>, issue: &mut Issue) -> bool {
    if c.op != "label_add" {
        let targets: Vec<String> = if c.op == "label_set" {
            issue.label_tags.keys().cloned().collect()
        } else {
            labels(c.data)
        };
        for label in targets {
            if let Some(tags) = issue.label_tags.get_mut(&label) {
                tags.retain(|tag| !c.k.saw(c.eid, *tag));
            }
        }
    }
    if c.op != "label_remove" {
        for label in labels(c.data) {
            issue.label_tags.entry(label).or_default().insert(c.eid);
        }
    }
    true
}

/// Whether a claim takes over the current one by naming it.
fn takes_over(claim: &Option<Claim>, data: &Object) -> bool {
    let named = data
        .get("takeover")
        .filter(|t| t.truthy())
        .and_then(Value::as_str)
        .and_then(EventId::parse);
    claim.as_ref().is_some_and(|held| named == Some(held.id))
}

/// A member of the event's actor, as the claim records it.
fn actor_text(event: &Event, key: &str) -> Option<String> {
    event
        .get("actor")
        .and_then(|actor| actor.get(key))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// The labels an event names.
fn labels(data: &Object) -> Vec<String> {
    let items = data
        .get("labels")
        .and_then(Value::as_array)
        .unwrap_or_default();
    items
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

/// A dangling dependency's type: `d.get("type") or "blocks"`.
fn dangling_type(dep: &Value) -> String {
    dep.get("type")
        .filter(|kind| kind.truthy())
        .map_or_else(|| "blocks".to_owned(), str_of)
}

/// Makes the issue a create names.
fn create(
    st: &mut State,
    k: &Knowledge,
    event: &Event,
    who: &str,
    when: &str,
) -> Result<(), KeySpace> {
    let (eid, data) = (event.id(), event.data());
    let mut key = data
        .get("key")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if st.by_key.contains_key(&key) {
        // Two machines minted the same key before they synced.
        let free = st.free_key(&key, eid)?;
        st.renames.push(Rename {
            from: std::mem::replace(&mut key, free.clone()),
            to: free,
            replica: event.replica().to_owned(),
            uid: eid,
        });
    }
    let mut issue = new_issue(event, key.clone(), who, when);
    st.by_key.insert(key, eid);
    materialise_dangling(k, &st.by_key, &mut issue);
    st.issues.insert(eid, issue);
    // An import may have named this key before it existed.
    let by_key = &st.by_key;
    for other in st.issues.values_mut() {
        if other.uid != eid && !other.dangling.is_empty() {
            materialise_dangling(k, by_key, other);
        }
    }
    Ok(())
}

/// The issue a create makes, under `key`: its defaults, then the fields it carries, with a register
/// for each, and what an import brings (labels, extras, comments, dependencies on keys).
fn new_issue(event: &Event, key: String, who: &str, when: &str) -> Issue {
    let (eid, data) = (event.id(), event.data());
    let empty = Object::new();
    let fields = data
        .get("fields")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let place = event.get("where").filter(|place| place.truthy()).cloned();
    let mut origin = Object::new();
    origin.insert("machine".into(), Value::from(event.replica()));
    for (name, value) in event
        .get("actor")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        origin.insert(name.clone(), value.clone());
    }
    if let Some(place) = &place {
        origin.insert("where".into(), place.clone());
    }
    let mut issue = Issue {
        uid: eid,
        key,
        fields: Default::default(),
        reg: IndexMap::new(),
        label_tags: labels(data)
            .into_iter()
            .map(|label| (label, BTreeSet::from([eid])))
            .collect(),
        dep_tags: IndexMap::new(),
        labels: BTreeSet::new(),
        deps: IndexMap::new(),
        comments: imported_comments(data),
        extra: data
            .get("extra")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default(),
        dangling: data
            .get("dangling")
            .and_then(Value::as_array)
            .unwrap_or_default()
            .to_vec(),
        retracted: BTreeMap::new(),
        claim: None,
        claim_generation: Int::from(0),
        claim_history: Vec::new(),
        provisional_claims: Vec::new(),
        lost_claims: Vec::new(),
        stale: Vec::new(),
        unconfirmed: Vec::new(),
        origin,
        where_: place,
        closed_on: None,
    };
    let defaults = [
        (Field::Title, Value::from("")),
        (Field::Status, Value::from("open")),
        (Field::Priority, Value::from(2)),
        (Field::IssueType, Value::from("task")),
        (Field::CreatedAt, Value::from(when)),
        (Field::CreatedBy, Value::from(who)),
        (Field::UpdatedAt, Value::from(when)),
    ];
    for (field, value) in defaults {
        issue.set(field, Some(value));
    }
    for (name, value) in fields {
        if let Some(field) = Field::from_name(name) {
            issue.set(field, Some(value.clone()));
        }
    }
    if data.get("imported").is_some_and(Value::truthy) && !fields.contains_key("created_by") {
        issue.set(Field::CreatedBy, Some(Value::Null)); // the record's author is unknown, not the importer
    }
    for field in REGISTERS {
        if let Some(value) = fields.get(field.name()) {
            issue
                .reg
                .insert(field, BTreeMap::from([(eid, value.clone())]));
        }
    }
    let mut status = Object::new();
    status.insert(
        "status".into(),
        issue.field(Field::Status).cloned().unwrap_or(Value::Null),
    );
    for field in [Field::ClosedAt, Field::CloseReason] {
        if let Some(value) = fields.get(field.name()) {
            status.insert(field.name().into(), value.clone());
        }
    }
    issue.reg.insert(
        Field::Status,
        BTreeMap::from([(eid, Value::Object(status))]),
    );
    issue
}

/// The comments an imported record carried, as Python keeps them: `{"uid": None, **comment}`.
fn imported_comments(data: &Object) -> Vec<Comment> {
    let comments = data
        .get("comments")
        .and_then(Value::as_array)
        .unwrap_or_default();
    comments
        .iter()
        .map(|comment| {
            let mut members = Object::new();
            members.insert("uid".into(), Value::Null);
            for (name, value) in comment.as_object().into_iter().flatten() {
                members.insert(name.clone(), value.clone());
            }
            Comment::Imported(members)
        })
        .collect()
}

/// Keeps every value its writer had not seen replaced; a write retires only what it observed (plus
/// anything it names explicitly, such as the claim a close ends).
fn write_register(
    k: &Knowledge,
    issue: &mut Issue,
    field: Field,
    eid: EventId,
    value: Value,
    supersede: &[EventId],
) {
    let register = issue.reg.entry(field).or_default();
    register.retain(|old, _| !(supersede.contains(old) || k.saw(eid, *old)));
    register.insert(eid, value);
    // Deterministic display; alternatives stay visible as a conflict.
    let shown = register
        .last_key_value()
        .map_or(Value::Null, |(_, shown)| shown.clone());
    if field == Field::Status {
        issue.set(Field::Status, shown.get("status").cloned());
        for extra in [Field::ClosedAt, Field::CloseReason] {
            issue.set(extra, shown.get(extra.name()).cloned());
        }
    } else if shown == Value::Null {
        issue.set(field, None);
    } else {
        issue.set(field, Some(shown));
    }
}

fn write_status(
    k: &Knowledge,
    issue: &mut Issue,
    eid: EventId,
    status: &str,
    when: &str,
    reason: Option<&str>,
    supersede: &[EventId],
) {
    let mut value = Object::new();
    value.insert("status".into(), Value::from(status));
    if DONE.contains(&status) {
        value.insert("closed_at".into(), Value::from(when));
        value.insert(
            "close_reason".into(),
            Value::from(reason.unwrap_or_default()),
        );
    }
    write_register(
        k,
        issue,
        Field::Status,
        eid,
        Value::Object(value),
        supersede,
    );
}

/// An imported dependency on a key that did not exist yet becomes a real edge the moment the key
/// does, tagged with the create that carried it, so every machine derives the same edge and a later
/// removal that saw that create can take it away.
fn materialise_dangling(k: &Knowledge, by_key: &IndexMap<String, EventId>, issue: &mut Issue) {
    let uid = issue.uid;
    let mut kept = Vec::new();
    for dep in std::mem::take(&mut issue.dangling) {
        let target = dep
            .get("depends_on_id")
            .and_then(Value::as_str)
            .and_then(|key| by_key.get(key));
        let Some(target) = target.copied().filter(|target| *target != uid) else {
            kept.push(dep);
            continue;
        };
        let (target, kind) = (target.to_string(), dangling_type(&dep));
        let removals = [(target.clone(), Some(kind.clone())), (target.clone(), None)];
        let taken_back = removals
            .iter()
            .filter_map(|removed| issue.retracted.get(removed))
            .flatten()
            .any(|removal| k.saw(*removal, uid));
        if taken_back {
            continue; // taken back before the target existed, by someone who had seen this import
        }
        let mut meta = Object::new(); // as dep_add stamps it
        meta.insert("created_at".into(), Value::from(event_time(uid.millis())));
        let creator = k
            .journal
            .event(uid)
            .map_or_else(String::new, crate::state::actor_of);
        meta.insert("created_by".into(), Value::from(creator));
        for name in ["created_at", "created_by", "metadata", "thread_id"] {
            if let Some(value) = dep.get(name) {
                meta.insert(name.into(), value.clone());
            }
        }
        issue
            .dep_tags
            .entry((target, kind))
            .or_default()
            .entry(uid)
            .or_insert(Value::Object(meta));
    }
    issue.dangling = kept;
}

fn end_claim(issue: &mut Issue, how: &'static str, who: &str, when: &str, reason: Option<&str>) {
    if let Some(claim) = issue.claim.take() {
        issue.claim_history.push(EndedClaim {
            claim,
            ended: when.to_owned(),
            ended_by: who.to_owned(),
            how,
            reason: reason
                .filter(|reason| !reason.is_empty())
                .map(str::to_owned),
        });
    }
}
