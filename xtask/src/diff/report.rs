//! The canonical report of a replayed journal, built from beadroll-core exactly as
//! `xtask/python/replay_dump.py` builds it from Python's replay.

use std::collections::BTreeMap;

use beadroll_core::event::Field;
use beadroll_core::id::EventId;
use beadroll_core::journal::{Journal, Problem, read_attempt, read_segment};
use beadroll_core::json::{Int, MAX_DIGITS, Object, Value};
use beadroll_core::query::{Edges, ReadyScope};
use beadroll_core::replay::replay;
use beadroll_core::state::{Issue, State};

/// One operation that builds a journal.
#[derive(Clone, Debug)]
pub(crate) enum Op {
    /// A segment's bytes, read into a machine's journal.
    Segment {
        replica: String,
        origin: String,
        data: Vec<u8>,
    },
    /// An attempt file's bytes, read unsequenced.
    Attempt {
        replica: String,
        origin: String,
        data: Vec<u8>,
    },
    /// One record added as it is.
    Add {
        event: Value,
        replica: String,
        origin: String,
        sequenced: bool,
    },
    /// A reader's problem, appended to the journal's list.
    Problem(Problem),
    /// The journal settled.
    Settle,
    /// A Beads JSONL text imported by a machine, all or nothing.
    Import {
        replica: String,
        path: String,
        text: String,
    },
}

/// The ids imports mint, from a counter, as the Python driver mints them.
struct Ids(u64);

impl Ids {
    fn next(&mut self) -> String {
        self.0 += 1;
        format!("019b0000-0000-7000-8000-{:012x}", self.0)
    }
}

/// An import's outcome, as the Python driver records it.
fn outcome(result: Result<usize, (&str, String)>) -> Value {
    match result {
        Ok(count) => obj([("imported", int(count))]),
        Err((code, message)) => obj([(
            "error",
            obj([("code", text(code)), ("message", text(message))]),
        )]),
    }
}

/// `text` imported into `journal` by `replica`, each event checked as `Tx.emit` checks it; the
/// journal is left as it was when the import fails.
fn import_into(
    journal: &mut Journal,
    replica: &str,
    path: &str,
    jsonl: &str,
    ids: &mut Ids,
) -> Result<usize, (&'static str, String)> {
    use beadroll_core::event::{MAX_NESTING, event_problem, payload_problem, write_problem};
    use beadroll_core::import::{Step, Target, plan_import, read_records};
    let validation = |error: String| ("VALIDATION", error);
    let records = read_records(jsonl, path).map_err(|error| validation(error.to_string()))?;
    // `tx.state` replays the Tx's journal, which settles it: the import works on that journal.
    let mut trial = journal.clone();
    trial.settle();
    let state = replay(trial.clone()).map_err(|error| ("KEY_SPACE", error.to_string()))?;
    let plan = plan_import(&state, &records).map_err(|error| validation(error.to_string()))?;
    let mut created: Vec<String> = Vec::new();
    for step in &plan.steps {
        let id = ids.next();
        let (issue, data) = match step {
            Step::DepRemove { issue, data } => (issue.to_string(), data.clone()),
            Step::Create(data) => (id.clone(), data.clone()),
            Step::DepAdd { issue, to, data } => {
                let to = match to {
                    Target::Issue(uid) => uid.to_string(),
                    Target::Created(at) => created.get(*at).cloned().unwrap_or_default(),
                };
                let mut full = Object::new();
                full.insert("to".into(), text(to));
                full.extend(data.clone());
                (created.get(*issue).cloned().unwrap_or_default(), full)
            }
        };
        let deps = list(trial.heads(), self::id);
        let event = obj([
            ("v", Value::from(1)),
            ("id", text(id.as_str())),
            ("replica", text(replica)),
            ("deps", deps),
            (
                "actor",
                obj([("agent", text("import")), ("thread", text("t"))]),
            ),
            ("op", text(step.op())),
            ("issue", text(issue)),
            ("data", Value::Object(data)),
        ]);
        if let Some(problem) = event_problem(&event)
            .or_else(|| payload_problem(&event))
            .or_else(|| write_problem(&event))
        {
            return Err(validation(problem));
        }
        if event.deeper_than(MAX_NESTING) {
            return Err(validation(format!(
                "data nests more than {MAX_NESTING} levels deep"
            )));
        }
        if matches!(step, Step::Create(_)) {
            created.push(id);
        }
        trial.add(event, replica, "", true);
    }
    *journal = trial;
    Ok(plan.imported)
}

/// Builds the journal the operations describe, and what each import came to.
pub(crate) fn journal_of(ops: &[Op]) -> (Journal, Vec<Value>) {
    let mut journal = Journal::new();
    let (mut ids, mut imports) = (Ids(0), Vec::new());
    for op in ops {
        match op {
            Op::Segment {
                replica,
                origin,
                data,
            } => {
                for event in read_segment(data, origin, Some(&mut journal.invalid)) {
                    journal.add(event, replica, origin, true);
                }
            }
            Op::Attempt {
                replica,
                origin,
                data,
            } => {
                for event in read_attempt(data, origin, Some(&mut journal.invalid)) {
                    journal.add(event, replica, origin, false);
                }
            }
            Op::Add {
                event,
                replica,
                origin,
                sequenced,
            } => {
                journal.add(event.clone(), replica, origin, *sequenced);
            }
            Op::Problem(problem) => journal.invalid.push(problem.clone()),
            Op::Settle => journal.settle(),
            Op::Import {
                replica,
                path,
                text: jsonl,
            } => {
                imports.push(outcome(import_into(
                    &mut journal,
                    replica,
                    path,
                    jsonl,
                    &mut ids,
                )));
            }
        }
    }
    (journal, imports)
}

fn obj<const N: usize>(members: [(&str, Value); N]) -> Value {
    Value::Object(
        members
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect(),
    )
}

fn text(text: impl Into<String>) -> Value {
    Value::Str(text.into())
}

fn id(id: EventId) -> Value {
    text(id.to_string())
}

fn list<T>(items: impl IntoIterator<Item = T>, each: impl Fn(T) -> Value) -> Value {
    Value::Array(items.into_iter().map(each).collect())
}

fn int(n: impl TryInto<i64>) -> Value {
    Value::Int(Int::from(n.try_into().unwrap_or(i64::MAX)))
}

fn opt(value: Option<&Value>) -> Value {
    value.cloned().unwrap_or(Value::Null)
}

/// The report of the journal the operations build.
pub(crate) fn report(ops: &[Op]) -> Value {
    let (journal, imports) = journal_of(ops);
    let st = match replay(journal) {
        Ok(st) => st,
        Err(error) => {
            let error = obj([
                ("code", text("KEY_SPACE")),
                ("message", text(error.to_string())),
            ]);
            return plain(obj([("error", error), ("imports", Value::Array(imports))]));
        }
    };
    let mut out = journal_part(st.journal());
    state_part(&mut out, &st);
    out.insert("imports".into(), Value::Array(imports));
    pairs(&mut out, &st);
    plain(Value::Object(out))
}

/// What the journal holds once replay has settled it.
fn journal_part(journal: &Journal) -> Object {
    let mut out = Object::new();
    let events = list(journal.events().iter(), |(eid, event)| {
        Value::Array(vec![id(*eid), event.value().clone()])
    });
    out.insert("events".into(), events);
    let order = journal
        .order()
        .iter()
        .map(|(replica, ids)| (replica.clone(), list(ids.iter(), |e| id(*e))));
    out.insert("order".into(), Value::Object(order.collect()));
    let quarantined = journal
        .quarantined()
        .iter()
        .map(|(eid, variants)| (eid.to_string(), Value::Array(variants.clone())));
    out.insert("quarantined".into(), Value::Object(quarantined.collect()));
    let invalid = list(journal.invalid.iter(), |p| {
        obj([
            ("event", p.event.clone().into()),
            ("origin", text(p.origin.as_str())),
            ("problem", text(p.problem.as_str())),
        ])
    });
    out.insert("invalid".into(), invalid);
    out.insert("heads".into(), list(journal.frontier(), id));
    out
}

/// What replay derived, and every query and view of it.
fn state_part(out: &mut Object, st: &State) {
    out.insert("prefix".into(), opt(st.prefix()));
    out.insert("name".into(), opt(st.name()));
    let integrity = list(st.integrity(), |row| {
        let mut members = Object::new();
        members.insert("event".into(), row.event.clone().into());
        members.insert("issue".into(), row.issue.clone().into());
        members.insert("problem".into(), text(row.problem.as_str()));
        if let Some(origin) = &row.origin {
            members.insert("origin".into(), text(origin.as_str()));
        }
        Value::Object(members)
    });
    out.insert("integrity".into(), integrity);
    out.insert("pending".into(), list(st.pending(), |e| id(*e)));
    let renames = list(st.renames(), |r| {
        obj([
            ("from", text(r.from.as_str())),
            ("to", text(r.to.as_str())),
            ("replica", text(r.replica.as_str())),
            ("uid", id(r.uid)),
        ])
    });
    out.insert("renames".into(), renames);
    let breaks = st
        .chain_breaks()
        .iter()
        .map(|(replica, at)| (replica.clone(), list(at, |n| int(*n))));
    out.insert("chain_breaks".into(), Value::Object(breaks.collect()));
    out.insert("orphans".into(), int(st.orphans()));
    out.insert("comment_count".into(), int(st.comment_count()));
    let last = list(st.last_by_thread(), |((replica, thread), eid)| {
        Value::Array(vec![
            text(replica.as_str()),
            thread.clone().into(),
            id(*eid),
        ])
    });
    out.insert("last_by_thread".into(), last);
    out.insert(
        "by_key".into(),
        list(st.keys(), |(key, uid)| {
            Value::Array(vec![text(key.as_str()), id(*uid)])
        }),
    );
    out.insert("issues".into(), list(st.issues(), issue_dump));
    out.insert("views".into(), list(st.issues(), |issue| views(st, issue)));
    out.insert(
        "ready".into(),
        list(st.ready(ReadyScope::All), |i| text(i.key())),
    );
    let cycles = |edges| list(st.cycles(edges), |cycle| list(cycle, text));
    out.insert("cycles".into(), cycles(Edges::WaitsFor));
    out.insert("cycles_blocking".into(), cycles(Edges::Blocking));
    out.insert("export".into(), list(st.export_records(), Value::Object));
}

/// For small journals, every pair: which event's writer saw which, and which issue waits for which.
fn pairs(out: &mut Object, st: &State) {
    let journal = st.journal();
    let applied: Vec<EventId> = journal
        .events()
        .keys()
        .filter(|e| !st.pending().contains(e))
        .copied()
        .collect();
    if applied.len() <= 25 {
        let pairs = applied
            .iter()
            .flat_map(|a| applied.iter().map(move |b| (*a, *b)));
        let saw = pairs.filter(|(a, b)| a != b && st.saw(*a, *b));
        out.insert(
            "saw".into(),
            list(saw, |(a, b)| Value::Array(vec![id(a), id(b)])),
        );
    }
    let uids: Vec<EventId> = st.issues().map(Issue::uid).collect();
    if uids.len() <= 12 {
        let pairs = uids.iter().flat_map(|a| uids.iter().map(move |b| (*a, *b)));
        let reaches = pairs.filter(|(a, b)| st.reaches(*a, *b));
        out.insert(
            "reaches".into(),
            list(reaches, |(a, b)| Value::Array(vec![id(a), id(b)])),
        );
    }
}

fn views(st: &State, issue: &Issue) -> Value {
    let keys = |issues: Vec<&Issue>| list(issues, |i| text(i.key()));
    obj([
        ("brief", Value::Object(st.brief(issue))),
        ("detail", Value::Object(st.detail(issue))),
        ("line", text(st.line(issue))),
        ("done", Value::Bool(st.is_done(issue))),
        ("blocked", Value::Bool(st.is_blocked(issue))),
        ("blockers", keys(st.open_blockers(issue))),
        ("parents", keys(st.parents_of(issue))),
        ("children", list(st.children(issue.uid()), |e| id(*e))),
        ("waits_for", list(st.waits_for(issue.uid()), id)),
        ("descendants", list(st.descendants(issue.uid()), id)),
        (
            "ready_beneath",
            keys(st.ready(ReadyScope::Subtree(issue.uid()))),
        ),
    ])
}

/// Every member of an issue, as the Python driver dumps it: sets sorted, tuple keys as lists,
/// members by name.
fn issue_dump(issue: &Issue) -> Value {
    let mut out: BTreeMap<String, Value> = BTreeMap::new();
    out.insert("uid".into(), id(issue.uid()));
    out.insert("id".into(), text(issue.key()));
    for field in Field::ALL {
        if let Some(value) = issue.field(field) {
            out.insert(field.name().into(), value.clone());
        }
    }
    let reg = issue.registers().iter().map(|(field, register)| {
        let register = register
            .iter()
            .map(|(eid, value)| (eid.to_string(), value.clone()));
        (field.name().to_owned(), Value::Object(register.collect()))
    });
    out.insert("reg".into(), Value::Object(reg.collect()));
    let label_tags = issue
        .label_tags()
        .iter()
        .map(|(label, tags)| (label.clone(), list(tags, |e| id(*e))));
    out.insert("label_tags".into(), Value::Object(label_tags.collect()));
    let dep_tags = list(issue.dep_tags(), |((to, kind), tags)| {
        let tags = tags
            .iter()
            .map(|(eid, meta)| (eid.to_string(), meta.clone()));
        Value::Array(vec![
            text(to.as_str()),
            text(kind.as_str()),
            Value::Object(tags.collect()),
        ])
    });
    out.insert("dep_tags".into(), dep_tags);
    out.insert("labels".into(), list(issue.labels(), |l| text(l.as_str())));
    let deps = list(issue.deps(), |((to, kind), meta)| {
        Value::Array(vec![text(to.as_str()), text(kind.as_str()), meta.clone()])
    });
    out.insert("deps".into(), deps);
    out.insert(
        "comments".into(),
        list(issue.comments(), |c| Value::Object(c.to_value())),
    );
    out.insert("extra".into(), Value::Object(issue.extra().clone()));
    out.insert("dangling".into(), Value::Array(issue.dangling().to_vec()));
    let retracted = list(issue.retracted(), |((to, kind), ids)| {
        Value::Array(vec![
            text(to.as_str()),
            kind.clone().into(),
            list(ids, |e| id(*e)),
        ])
    });
    out.insert("retracted".into(), retracted);
    out.insert(
        "claim".into(),
        issue
            .claim()
            .map_or(Value::Null, |c| Value::Object(c.to_value())),
    );
    out.insert(
        "claim_generation".into(),
        Value::Int(issue.claim_generation().clone()),
    );
    out.insert(
        "claim_history".into(),
        list(issue.claim_history(), |c| Value::Object(c.to_value())),
    );
    out.insert(
        "provisional_claims".into(),
        list(issue.provisional_claims(), |p| Value::Object(p.to_value())),
    );
    out.insert(
        "lost_claims".into(),
        list(issue.lost_claims(), |l| Value::Object(l.to_value())),
    );
    out.insert(
        "stale".into(),
        list(issue.stale(), |s| Value::Object(s.to_value())),
    );
    out.insert(
        "unconfirmed".into(),
        Value::Array(issue.unconfirmed().to_vec()),
    );
    out.insert("origin".into(), Value::Object(issue.origin().clone()));
    if let Some(place) = issue.where_() {
        out.insert("where".into(), place.clone());
    }
    if let Some(place) = issue.closed_on() {
        out.insert("closed_on".into(), place.clone());
    }
    Value::Object(out.into_iter().collect())
}

/// The report as JSON can hold it: an integer of more digits than Python prints by default is
/// written `{"big_int": DIGITS}`, as the Python driver writes it.
fn plain(value: Value) -> Value {
    match value {
        Value::Int(n) if n.digit_count() > MAX_DIGITS => obj([("big_int", text(n.to_string()))]),
        Value::Array(items) => Value::Array(items.into_iter().map(plain).collect()),
        Value::Object(members) => {
            Value::Object(members.into_iter().map(|(k, v)| (k, plain(v))).collect())
        }
        other => other,
    }
}
