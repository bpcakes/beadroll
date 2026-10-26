//! Applies a create and turns imported dependencies on its key into edges once the key exists.

use std::collections::{BTreeMap, BTreeSet};

use super::Doing;
use crate::event::{Field, REGISTERS};
use crate::id::EventId;
use crate::json::{Int, Object, Value};
use crate::op::Create;
use crate::state::{
    Comment, DepTags, Edge, IdMap, IdSet, Issue, KeySpace, Knowledge, Meta, Origin, Rename, Stamps,
    State, Target, VecMap, awaited,
};
use crate::time::event_time;

use super::{Names, dangling_type};

/// Makes the issue a create names.
pub(super) fn create(
    st: &mut State,
    k: &Knowledge,
    event: &Doing<'_>,
    create: &Create,
    (who, when): (&str, &str),
    names: &Names,
) -> Result<(), KeySpace> {
    let eid = event.id;
    let mut key = create.key.clone();
    if st.by_key.contains_key(&key) {
        // Two machines minted the same key before they synced.
        let free = st.free_key(&key, eid)?;
        st.renames.push(Rename {
            from: std::mem::replace(&mut key, free.clone()),
            to: free,
            replica: event.trace.replica().to_owned(),
            uid: eid,
        });
    }
    let mut issue = new_issue(event, create, key.clone(), who, when);
    st.by_key.insert(key.clone(), eid);
    // What the import names of keys that have their issues becomes edges now; the rest waits. Its
    // own key never settles what it names of itself.
    for dep in std::mem::take(&mut issue.dangling) {
        let target = (awaited(&dep))
            .and_then(|named| st.by_key.get(named))
            .filter(|target| **target != eid);
        match target {
            Some(target) => bind(k, names, &mut issue, &dep, *target),
            None => issue.dangling.push(dep),
        }
    }
    // An import may have named this key before it existed: the entries that wait for it become
    // edges, and nothing else waiting is visited (roll-lqjj.18). Each touches its own issue alone.
    // The key is an issue's now, so it stays claimed, and a run that holds it stays true
    // (`state::Runs`).
    for (uid, number) in st.awaiting.arrived(&key) {
        let Some(other) = st.issues.get_mut(&uid) else {
            continue;
        };
        if let Some(dep) = st.awaiting.take(other, number) {
            bind(k, names, other, &dep, eid);
        }
    }
    // Listed after the key's own entries are settled, so that what it names of itself waits.
    st.awaiting.wait(&issue);
    st.issues.insert(eid, Box::new(issue));
    Ok(())
}

/// The issue a create makes, under `key`: a register for each field it carries, the status's
/// holding its closed_at and close_reason too, its stamps (given, or of the create), and what an
/// import brings (labels, extras, comments, dependencies on keys).
fn new_issue(event: &Doing<'_>, create: &Create, key: String, who: &str, when: &str) -> Issue {
    let eid = event.id;
    let place = (event.what.place.as_ref())
        .filter(|place| place.truthy())
        .cloned();
    let origin = Origin {
        machine: std::sync::Arc::clone(event.trace.shared_replica()),
        actor: event.trace.shared_actor().cloned(),
        place: place.clone(),
    };
    let given = |field: Field| {
        create
            .fields
            .iter()
            .find(|(f, _)| *f == field)
            .map(|(_, value)| value.clone())
    };
    let stamps = Stamps {
        created_at: given(Field::CreatedAt).unwrap_or_else(|| Value::from(when)),
        // an import's record that does not say has an unknown author, not the importer
        created_by: given(Field::CreatedBy).unwrap_or_else(|| {
            if create.imported {
                Value::Null
            } else {
                Value::from(who)
            }
        }),
        updated_at: given(Field::UpdatedAt).unwrap_or_else(|| Value::from(when)),
        own_closed_at: false,
        own_close_reason: false,
    };
    // Each field it gives is kept in a register, which shows it, and nowhere else; a field it
    // does not give shows its default until a write gives it a register (roll-lqjj.41).
    let mut reg = VecMap::default();
    for field in REGISTERS {
        if let Some(value) = given(field) {
            reg.insert(field, IdMap::of(eid, value));
        }
    }
    let mut status = Object::new();
    let open = Value::from("open");
    status.insert("status".into(), given(Field::Status).unwrap_or(open));
    for field in [Field::ClosedAt, Field::CloseReason] {
        if let Some(value) = given(field) {
            status.insert(field.name().into(), value);
        }
    }
    reg.insert(Field::Status, IdMap::of(eid, Value::from(status)));
    Issue {
        uid: eid,
        key,
        stamps,
        reg,
        label_tags: create
            .labels
            .iter()
            .map(|label| (label.clone(), IdSet::of(eid)))
            .collect(),
        dep_tags: DepTags::default(),
        labels: BTreeSet::new(),
        comments: create.comments.iter().map(imported_comment).collect(),
        extra: (!create.extra.is_empty()).then(|| Box::new(create.extra.clone())),
        dangling: create.dangling.clone(),
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
        texts: crate::state::Texts::Here,
    }
}

/// An imported comment with uid defaulting to null, followed by the imported fields.
fn imported_comment(comment: &Object) -> Comment {
    let mut members = Object::new();
    members.insert("uid".into(), Value::Null);
    members.extend(
        comment
            .iter()
            .map(|(name, value)| (name.clone(), value.clone())),
    );
    Comment::Imported(members)
}

/// An imported dependency on a key that did not exist yet becomes a real edge to `target`, the issue
/// that has the key, the moment the key exists, tagged with the create that carried it, so every
/// machine derives the same edge and a later removal that saw that create can take it away.
fn bind(k: &Knowledge, names: &Names, issue: &mut Issue, dep: &Value, target: EventId) {
    let uid = issue.uid;
    let kind = dangling_type(dep);
    let text = target.to_string();
    let removals = [(text.clone(), Some(kind.clone())), (text, None)];
    let taken_back = removals
        .iter()
        .filter_map(|removed| issue.retracted.get(removed))
        .flatten()
        .any(|removal| k.saw(*removal, uid));
    if taken_back {
        return; // taken back before the target existed, by someone who had seen this import
    }
    let mut meta = Object::new(); // as dep_add stamps it
    meta.insert("created_at".into(), Value::from(event_time(uid.millis())));
    let creator = k.who(uid).unwrap_or_default();
    meta.insert("created_by".into(), Value::from(creator));
    for name in ["created_at", "created_by", "metadata", "thread_id"] {
        if let Some(value) = dep.get(name) {
            meta.insert(name.into(), value.clone());
        }
    }
    let edge = Edge {
        to: Target::Id(target),
        kind: names.of(&kind),
    };
    issue
        .dep_tags
        .added(edge)
        .insert_if_absent(uid, Meta::of(meta));
}
