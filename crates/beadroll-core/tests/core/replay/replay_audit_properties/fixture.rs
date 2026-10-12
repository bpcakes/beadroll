//! The first audit's fixture (`ReplayAuditFixture`): a history of an `init` and the issue
//! `au-one` on machine `seed`, the events a test adds, and the snapshot of what a reader sees,
//! which `ReplayAuditProperties` and `ReplayAuditBugs` share.

use beadroll_core::event::Field;
use beadroll_core::id::EventId;
use beadroll_core::journal::Journal;
use beadroll_core::json::{Format, Object, write};
use beadroll_core::query::Alternative;
use beadroll_core::replay::replay;
use beadroll_core::state::{Integrity, Issue, State};
use beadroll_test_support::events::journal;
use beadroll_test_support::{Json, json};

/// A cause no journal holds.
pub(crate) const MISSING: &str = "6f2c1b9a-3d4e-4f50-8a61-7b8c9d0e1f2a";

/// The audit's history: an `init` and the issue `au-one` on machine `seed`, then whatever a test
/// adds. Ids carry the event's number in their clock and their random bits, so they sort as made.
pub(crate) struct Fixture {
    pub(crate) events: Vec<Json>,
    counter: u128,
    /// The id of `au-one`'s create.
    pub(crate) issue: String,
}

impl Fixture {
    pub(crate) fn new() -> Fixture {
        let mut f = Fixture {
            events: Vec::new(),
            counter: 0,
            issue: String::new(),
        };
        f.event(
            "init",
            None,
            "seed",
            &[],
            json!({"prefix": "au", "name": "audit"}),
        );
        f.issue = f.create("au-one");
        f
    }

    /// Records an event as the audit wrote them; returns its id.
    pub(crate) fn event(
        &mut self,
        op: &str,
        issue: Option<&str>,
        replica: &str,
        deps: &[&str],
        data: Json,
    ) -> String {
        self.counter += 1;
        let value =
            ((1_790_000_000_000 + self.counter) << 80) | (7 << 76) | (2 << 62) | self.counter;
        let eid = EventId::from_u128(value).to_string();
        let deps = Json::Array(deps.iter().map(|d| Json::from(*d)).collect());
        self.events.push(json!({
            "v": 1,
            "id": (eid.as_str()),
            "replica": replica,
            "deps": deps,
            "actor": {"agent": "codex", "thread": (format!("{replica}-thread"))},
            "op": op,
            "issue": (issue.unwrap_or(&eid)),
            "data": data
        }));
        eid
    }

    /// A create on `seed` after the last event, its title its key.
    pub(crate) fn create(&mut self, key: &str) -> String {
        let last = self.last();
        self.event(
            "create",
            None,
            "seed",
            &[&last],
            json!({"key": key, "fields": {"title": key}}),
        )
    }

    /// The id of the last event.
    pub(crate) fn last(&self) -> String {
        self.events
            .last()
            .and_then(|e| e.get("id"))
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_owned()
    }

    /// Modifies the recorded event `eid` in place.
    pub(crate) fn edit(&mut self, eid: &str, change: impl FnOnce(&mut Object)) {
        let event = self
            .events
            .iter_mut()
            .find(|e| e.get("id").and_then(Json::as_str) == Some(eid));
        if let Some(Json::Object(members)) = event {
            change(members);
        }
    }

    /// Every event, each in its own machine's journal (an event that names none in machine `a`'s).
    pub(crate) fn journal(&self) -> Journal {
        journal(&self.events)
    }

    pub(crate) fn state(&self) -> State {
        replay(self.journal()).unwrap()
    }
}

/// A `set` payload writing one field.
pub(crate) fn fields(field: Field, value: Json) -> Json {
    let mut fields = Object::new();
    fields.insert(field.name().into(), value);
    json!({"fields": (Json::from(fields))})
}

/// The issue `uid`.
pub(crate) fn issue<'a>(st: &'a State, uid: &str) -> &'a Issue {
    st.issue_named(uid).unwrap()
}

/// The alternatives of a disputed field, if it is disputed.
pub(crate) fn conflict(st: &State, issue: &Issue, field: Field) -> Option<Vec<Alternative>> {
    st.conflicts(issue)
        .into_iter()
        .find(|(f, _)| *f == field)
        .map(|(_, alternatives)| alternatives)
}

fn integrity_row(row: &Integrity) -> Json {
    let mut out = json!({"event": (row.event.clone()), "issue": (row.issue.clone()), "problem": (row.problem.as_str())});
    if let (Json::Object(members), Some(origin)) = (&mut out, &row.origin) {
        members.insert("origin".into(), Json::from(origin.as_str()));
    }
    out
}

/// Everything a reader sees of a journal: the export, every issue in full, what is ready,
/// integrity and pending events.
pub(crate) fn snapshot(journal: Journal) -> String {
    let st = replay(journal).unwrap();
    let issues = st.issues_by_key();
    let export: Vec<Json> = st.export_records().into_iter().map(Json::from).collect();
    let details: Vec<Json> = issues.iter().map(|i| Json::from(st.detail(i))).collect();
    let ready: Vec<Json> = issues
        .iter()
        .filter(|i| i.status() == "open" && !st.is_blocked(i) && !st.disputed(i, Field::Status))
        .map(|i| Json::from(i.key()))
        .collect();
    let integrity: Vec<Json> = st.integrity().iter().map(integrity_row).collect();
    let mut pending: Vec<String> = st.pending().iter().map(ToString::to_string).collect();
    pending.sort();
    let snapshot = json!({
        "export": (Json::Array(export)),
        "details": (Json::Array(details)),
        "ready": (Json::Array(ready)),
        "integrity": (Json::Array(integrity)),
        "pending": (Json::Array(pending.into_iter().map(Json::from).collect()))
    });
    write(&snapshot, Format::CANONICAL).unwrap()
}
