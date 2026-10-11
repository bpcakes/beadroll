//! One event applied to the state (Python's `apply_event`, `write_register` and `write_status`),
//! branch for branch, on the event's typed operation: ownership in [`claims`], a create and the
//! imported dependencies it brings in [`create`].

mod claims;
mod create;

use std::collections::HashSet;
use std::sync::Arc;

use indexmap::IndexMap;

use crate::event::{DONE, Field};
use crate::id::EventId;
use crate::journal::Event;
use crate::json::{Object, Value};
use crate::op::Op;
use crate::state::{Comment, IdMap, IdSet, Issue, KeySpace, Knowledge, Stale, State};
use crate::text::str_of;

/// Applies one event, the journal's event at `at`. `Ok(true)` when it touched an existing issue.
pub(super) fn apply_event(
    st: &mut State,
    k: &Knowledge,
    at: usize,
    event: &Event,
    who: &str,
    when: &str,
    superseded: &HashSet<EventId>,
) -> Result<bool, KeySpace> {
    let op = match event.op() {
        Op::Init { prefix, name } => {
            if st.prefix.is_none() {
                st.prefix = prefix.as_deref().cloned();
                st.name = name.as_deref().cloned();
            }
            return Ok(false);
        }
        Op::Create(making) => {
            create::create(st, k, event, making, who, when)?;
            return Ok(false);
        }
        other => other,
    };
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
        at,
        eid: event.id(),
        uid,
        who,
        when,
        place: event.shared_place().filter(|place| place.truthy()),
        claim: issue.claim.as_ref().map(|held| Held {
            id: held.id,
            mine: held.owner == who,
        }),
        superseded,
    };
    let applied = match op {
        Op::Set(fields) => set(&c, issue, fields),
        Op::Claim(claiming) => claims::claim(&c, issue, claiming),
        Op::Release(ending) => claims::release(&c, issue, ending),
        Op::Close(ending) => claims::close(&c, issue, ending, true),
        Op::Delete(ending) => claims::close(&c, issue, ending, false),
        Op::Reopen => claims::reopen(&c, issue),
        Op::Comment { text, author } => {
            st.comment_count += 1;
            comment(&c, issue, st.comment_count, text, author.as_deref())
        }
        Op::DepAdd { to, kind, meta } => dep_add(&c, issue, to, kind.as_deref(), meta),
        Op::DepRemove { to, key, kind } => dep_remove(
            &c,
            issue,
            &st.by_key,
            to.as_deref(),
            key.as_deref(),
            kind.as_deref(),
        ),
        Op::LabelAdd(labels) => label(&c, issue, labels, Labels::Add),
        Op::LabelRemove(labels) => label(&c, issue, labels, Labels::Remove),
        Op::LabelSet(labels) => label(&c, issue, labels, Labels::Set),
        // Anything else that reaches here applies nothing, but touches the issue: a placeholder
        // whose position broke its journal's chain is not inert (Python's replay).
        Op::Init { .. } | Op::Create(_) | Op::Quarantined | Op::Invalid | Op::Unknown(_) => true,
    };
    Ok(applied)
}

/// What every operation reads: the event, its writer and time, and the issue's claim before it.
struct Ctx<'a> {
    k: &'a Knowledge,
    event: &'a Event,
    /// The event's index in the journal.
    at: usize,
    eid: EventId,
    uid: EventId,
    who: &'a str,
    when: &'a str,
    /// The event's `where`, when it names a place, shared with the events written alike.
    place: Option<&'a Arc<Value>>,
    claim: Option<Held>,
    superseded: &'a HashSet<EventId>,
}

/// The issue's claim before the event: all an operation asks of it.
#[derive(Clone, Copy)]
struct Held {
    /// The claim event.
    id: EventId,
    /// Whether the event's writer is its owner.
    mine: bool,
}

impl Ctx<'_> {
    /// Records an end of a claim that was no longer current; it applies nothing.
    fn stale(&self, issue: &mut Issue, named: Option<&str>) -> bool {
        issue.stale.push(Stale {
            op: self.event.op_name().to_owned(),
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
            self.k,
            issue,
            (self.at, self.eid),
            status,
            self.when,
            reason,
            supersede,
        );
    }

    fn write(&self, issue: &mut Issue, field: Field, value: Value, supersede: &[EventId]) {
        write_register(self.k, issue, field, (self.at, self.eid), value, supersede);
    }
}

fn set(c: &Ctx<'_>, issue: &mut Issue, fields: &[(Field, Value)]) -> bool {
    for (field, value) in fields {
        if *field == Field::Status {
            c.write_status(issue, value.as_str().unwrap_or_default(), None, &[]);
        } else {
            c.write(issue, *field, value.clone(), &[]);
        }
    }
    true
}

/// A comment, by `author` when it names one (`data.get("author") or who`).
fn comment(c: &Ctx<'_>, issue: &mut Issue, seq: u64, text: &str, author: Option<&str>) -> bool {
    let author = author.filter(|a| !a.is_empty()).unwrap_or(c.who).to_owned();
    issue.comments.push(Comment::Native {
        uid: c.eid,
        seq,
        author,
        text: text.to_owned(),
        created_at: c.when.to_owned(),
        where_: c.place.cloned(),
    });
    true
}

/// A dependency on `to` of type `kind` (`blocks` when none), stamped like Python's, `meta` over it.
fn dep_add(c: &Ctx<'_>, issue: &mut Issue, to: &str, kind: Option<&str>, meta: &Object) -> bool {
    let kind = kind
        .filter(|kind| !kind.is_empty())
        .unwrap_or("blocks")
        .to_owned();
    let mut stamp = Object::new();
    stamp.insert("created_at".into(), Value::from(c.when));
    stamp.insert("created_by".into(), Value::from(c.who));
    stamp.extend(
        meta.iter()
            .map(|(name, value)| (name.clone(), value.clone())),
    );
    issue
        .dep_tags
        .get_or_insert_with((to.to_owned(), kind), IdMap::default)
        .insert(c.eid, Value::Object(stamp));
    true
}

/// Removes only the additions its writer had seen: by `key`, an imported dependency on a key that
/// was not an issue for its writer; otherwise the edges to `to`, of type `wanted` or of any type.
fn dep_remove(
    c: &Ctx<'_>,
    issue: &mut Issue,
    by_key: &IndexMap<String, EventId>,
    to: Option<&str>,
    key: Option<&str>,
    wanted: Option<&str>,
) -> bool {
    let matches = |kind: &str| wanted.is_none_or(|wanted| wanted == kind);
    if let Some(key) = key.filter(|key| !key.is_empty()) {
        issue.dangling.retain(|dep| {
            let depends_on = dep.get("depends_on_id").and_then(Value::as_str);
            !(depends_on == Some(key) && matches(&dangling_type(dep)))
        });
        // A concurrent create of the key that sorted first bound it already.
        if let Some(bound) = by_key.get(key).filter(|_| c.k.saw_at(c.at, c.uid)) {
            let bound = bound.to_string();
            for ((to, kind), tags) in issue.dep_tags.iter_mut() {
                if *to == bound && matches(kind) {
                    tags.remove(&c.uid); // the import's own tag only: an explicit `dep add` keeps its edge
                }
            }
        }
    } else {
        let to = to.unwrap_or_default();
        for ((edge_to, kind), tags) in issue.dep_tags.iter_mut() {
            if edge_to == to && matches(kind) {
                tags.retain(|tag, _| !c.k.saw_at(c.at, *tag));
            }
        }
        let removed = (to.to_owned(), wanted.map(str::to_owned));
        issue.retracted.entry(removed).or_default().insert(c.eid);
    }
    true
}

/// Which label operation.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Labels {
    Add,
    Remove,
    Set,
}

/// Labels added, removed (what the writer saw of them), or set: every label the writer saw removed,
/// then these added.
fn label(c: &Ctx<'_>, issue: &mut Issue, labels: &[String], how: Labels) -> bool {
    if how != Labels::Add {
        let targets: Vec<String> = if how == Labels::Set {
            issue.label_tags.keys().cloned().collect()
        } else {
            labels.to_vec()
        };
        for label in targets {
            if let Some(tags) = issue.label_tags.get_mut(&label) {
                tags.retain(|tag| !c.k.saw_at(c.at, *tag));
            }
        }
    }
    if how != Labels::Remove {
        for label in labels {
            issue
                .label_tags
                .get_or_insert_with(label.clone(), IdSet::default)
                .insert(c.eid);
        }
    }
    true
}

/// A dangling dependency's type: `d.get("type") or "blocks"`.
fn dangling_type(dep: &Value) -> String {
    dep.get("type")
        .filter(|kind| kind.truthy())
        .map_or_else(|| "blocks".to_owned(), str_of)
}

/// Keeps every value its writer had not seen replaced; a write retires only what it observed (plus
/// anything it names explicitly, such as the claim a close ends).
fn write_register(
    k: &Knowledge,
    issue: &mut Issue,
    field: Field,
    (at, eid): (usize, EventId),
    value: Value,
    supersede: &[EventId],
) {
    let register = issue.reg.get_or_insert_with(field, IdMap::default);
    register.retain(|old, _| !(supersede.contains(old) || k.saw_at(at, *old)));
    register.insert(eid, value);
    // Deterministic display; alternatives stay visible as a conflict. Only what is shown is
    // copied out of the register.
    let shown = register.last().map(|(_, shown)| shown);
    if field == Field::Status {
        let member = |name: &str| shown.and_then(|shown| shown.get(name)).cloned();
        let shown = [
            (Field::Status, member("status")),
            (Field::ClosedAt, member(Field::ClosedAt.name())),
            (Field::CloseReason, member(Field::CloseReason.name())),
        ];
        for (field, value) in shown {
            issue.set(field, value);
        }
    } else {
        let shown = shown.filter(|shown| **shown != Value::Null).cloned();
        issue.set(field, shown);
    }
}

fn write_status(
    k: &Knowledge,
    issue: &mut Issue,
    (at, eid): (usize, EventId),
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
        (at, eid),
        Value::Object(value),
        supersede,
    );
}
