//! Applies a create and turns imported dependencies on its key into edges once the key exists.

use std::collections::{BTreeMap, BTreeSet};

use indexmap::IndexMap;

use super::Doing;
use crate::event::{Field, REGISTERS};
use crate::id::EventId;
use crate::json::{Int, Object, Value};
use crate::op::Create;
use crate::state::{
    Comment, IdMap, IdSet, Issue, KeySpace, Knowledge, Origin, Rename, State, VecMap, is_text,
};
use crate::time::event_time;

use super::dangling_type;

/// Makes the issue a create names.
pub(super) fn create(
    st: &mut State,
    k: &Knowledge,
    event: &Doing<'_>,
    create: &Create,
    who: &str,
    when: &str,
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
    st.by_key.insert(key, eid);
    materialise_dangling(k, &st.by_key, &mut issue);
    let waits = !issue.dangling.is_empty();
    st.issues.insert(eid, Box::new(issue));
    // An import may have named this key before it existed. Each issue's materialising touches that
    // issue alone, so the issues still waiting are all there is to visit, in any order.
    let (by_key, issues) = (&st.by_key, &mut st.issues);
    st.awaiting.retain(|uid| {
        let Some(other) = issues
            .get_mut(uid)
            .filter(|other| !other.dangling.is_empty())
        else {
            return false;
        };
        materialise_dangling(k, by_key, other);
        !other.dangling.is_empty()
    });
    if waits {
        st.awaiting.push(eid);
    }
    Ok(())
}

/// The issue a create makes, under `key`: its defaults, then the fields it carries, with a register
/// for each, and what an import brings (labels, extras, comments, dependencies on keys).
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
    let mut issue = Issue {
        uid: eid,
        key,
        fields: Default::default(),
        reg: VecMap::default(),
        label_tags: create
            .labels
            .iter()
            .map(|label| (label.clone(), IdSet::of(eid)))
            .collect(),
        dep_tags: VecMap::default(),
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
    // a long field is kept in its register alone, below, and shown from there
    for (field, value) in create.fields.iter().filter(|(field, _)| !is_text(*field)) {
        issue.set(*field, Some(value.clone()));
    }
    let given = |field: Field| {
        create
            .fields
            .iter()
            .find(|(f, _)| *f == field)
            .map(|(_, value)| value)
    };
    if create.imported && given(Field::CreatedBy).is_none() {
        issue.set(Field::CreatedBy, Some(Value::Null)); // the record's author is unknown, not the importer
    }
    for field in REGISTERS {
        if let Some(value) = given(field) {
            issue.reg.insert(field, IdMap::of(eid, value.clone()));
        }
    }
    let mut status = Object::new();
    status.insert(
        "status".into(),
        issue.field(Field::Status).cloned().unwrap_or(Value::Null),
    );
    for field in [Field::ClosedAt, Field::CloseReason] {
        if let Some(value) = given(field) {
            status.insert(field.name().into(), value.clone());
        }
    }
    issue
        .reg
        .insert(Field::Status, IdMap::of(eid, Value::from(status)));
    issue
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
        let creator = k.who(uid).unwrap_or_default();
        meta.insert("created_by".into(), Value::from(creator));
        for name in ["created_at", "created_by", "metadata", "thread_id"] {
            if let Some(value) = dep.get(name) {
                meta.insert(name.into(), value.clone());
            }
        }
        issue
            .dep_tags
            .get_or_insert_with((target, kind), IdMap::default)
            .insert_if_absent(uid, Value::from(meta));
    }
    issue.dangling = kept;
}
