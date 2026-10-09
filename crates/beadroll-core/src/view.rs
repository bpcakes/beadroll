//! How an issue is shown (Python's `brief`, `detail` and `line`): objects in Python's key order, and
//! the one-line form `list` prints.

use crate::event::{EXPORT_FIELDS, Field};
use crate::json::{Object, Value};
use crate::query::Alternative;
use crate::state::{Comment, Issue, State};
use crate::text::str_of;

/// The status icons `list` prints.
pub fn icon(status: &str) -> &'static str {
    match status {
        "open" => "○",
        "in_progress" => "◐",
        "blocked" => "●",
        "deferred" => "❄",
        "closed" => "✓",
        "tombstone" => "✗",
        _ => "?",
    }
}

/// A field as Python's `issue[name]` reads it (`null` when absent).
fn field(issue: &Issue, name: Field) -> Value {
    issue.field(name).cloned().unwrap_or(Value::Null)
}

/// The branch an issue was last worked on, when it names one.
fn branch(place: Option<&Value>) -> Option<&Value> {
    place
        .and_then(|place| place.get("branch"))
        .filter(|branch| branch.truthy())
}

/// The names of the disputed fields, sorted.
fn disputed(st: &State, issue: &Issue) -> Vec<String> {
    let mut names: Vec<String> = st
        .conflicts(issue)
        .into_iter()
        .map(|(field, _)| field.name().to_owned())
        .collect();
    names.sort();
    names
}

impl State {
    /// An issue in a list (Python's `brief`).
    pub fn brief(&self, issue: &Issue) -> Object {
        let mut out = Object::new();
        out.insert("id".into(), Value::from(issue.key()));
        for name in [
            Field::Title,
            Field::Status,
            Field::Priority,
            Field::IssueType,
        ] {
            out.insert(name.name().into(), field(issue, name));
        }
        if let Some(assignee) = issue.get(Field::Assignee).filter(|a| a.truthy()) {
            out.insert("assignee".into(), assignee.clone());
        }
        if !issue.labels().is_empty() {
            out.insert(
                "labels".into(),
                issue
                    .labels()
                    .iter()
                    .map(|l| Value::from(l.as_str()))
                    .collect::<Vec<_>>()
                    .into(),
            );
        }
        if let Some(branch) = branch(issue.where_()) {
            out.insert("branch".into(), branch.clone());
        }
        let disputed = disputed(self, issue);
        if !disputed.is_empty() {
            out.insert(
                "conflicted".into(),
                disputed
                    .into_iter()
                    .map(Value::from)
                    .collect::<Vec<_>>()
                    .into(),
            );
        }
        out.insert("created_at".into(), field(issue, Field::CreatedAt));
        out.insert("updated_at".into(), field(issue, Field::UpdatedAt));
        out
    }

    /// An issue in full (Python's `detail` without the merge check, which needs git: the CLI adds
    /// `merged` after `closed_on`).
    pub fn detail(&self, issue: &Issue) -> Object {
        let mut out = Object::new();
        out.insert("id".into(), Value::from(issue.key()));
        for name in EXPORT_FIELDS {
            if let Some(value) = issue.get(name).filter(|value| **value != Value::from("")) {
                out.insert(name.name().into(), value.clone());
            }
        }
        if !issue.labels().is_empty() {
            out.insert(
                "labels".into(),
                texts(issue.labels().iter().map(String::as_str)),
            );
        }
        let parents = self.parents_of(issue);
        if let Some(first) = parents.first() {
            out.insert("parent".into(), Value::from(first.key()));
            if parents.len() > 1 {
                out.insert("parents".into(), texts(parents.iter().map(|p| p.key())));
            }
        }
        out.insert("dependencies".into(), self.dependencies_shown(issue));
        if !issue.dangling().is_empty() {
            out.insert("unresolved_dependencies".into(), unresolved(issue));
        }
        out.insert("dependents".into(), self.dependents(issue));
        let comments = issue
            .comments()
            .iter()
            .map(|comment| Value::Object(shown_comment(comment)));
        out.insert("comments".into(), comments.collect::<Vec<_>>().into());
        out.insert("blocked".into(), Value::from(self.is_blocked(issue)));
        out.insert("origin".into(), Value::Object(issue.origin().clone()));
        if let Some(place) = issue.where_().filter(|place| place.truthy()) {
            out.insert("where".into(), place.clone());
        }
        if let Some(place) = issue.closed_on().filter(|place| place.truthy()) {
            out.insert("closed_on".into(), place.clone());
        }
        if let Some(claim) = issue.claim() {
            out.insert("claim".into(), Value::Object(claim.to_value()));
        }
        let objects = |items: Vec<Object>| items.into_iter().map(Value::Object).collect::<Vec<_>>();
        let lists = [
            (
                "provisional_claims",
                objects(
                    issue
                        .provisional_claims()
                        .iter()
                        .map(|p| p.to_value())
                        .collect(),
                ),
            ),
            (
                "lost_claims",
                objects(issue.lost_claims().iter().map(|l| l.to_value()).collect()),
            ),
            (
                "stale",
                objects(issue.stale().iter().map(|s| s.to_value()).collect()),
            ),
            ("unconfirmed", issue.unconfirmed().to_vec()),
        ];
        for (name, items) in lists {
            if !items.is_empty() {
                out.insert(name.into(), items.into());
            }
        }
        let conflicts = self.conflicts(issue);
        if !conflicts.is_empty() {
            let conflicts = conflicts.into_iter().map(|(field, alternatives)| {
                let alternatives =
                    objects(alternatives.iter().map(Alternative::to_value).collect());
                (field.name().to_owned(), Value::from(alternatives))
            });
            out.insert("conflicts".into(), Value::Object(conflicts.collect()));
        }
        out
    }

    /// What an issue depends on, by sorted edge: the key, title and status of each target (an
    /// edge to text that names no issue shows it with status `unknown`).
    fn dependencies_shown(&self, issue: &Issue) -> Value {
        let mut edges: Vec<&(String, String)> = issue.deps().keys().collect();
        edges.sort();
        let dependencies = edges.iter().map(|(to, kind)| {
            let target = self.issue_named(to);
            let mut dep = Object::new();
            dep.insert("id".into(), Value::from(self.key_of(to)));
            dep.insert("title".into(), Value::from(target.map_or("", Issue::title)));
            dep.insert(
                "status".into(),
                Value::from(target.map_or("unknown", Issue::status)),
            );
            dep.insert("dependency_type".into(), Value::from(kind.as_str()));
            Value::Object(dep)
        });
        dependencies.collect::<Vec<_>>().into()
    }

    /// The issues that depend on this one, in replay order, each by sorted edge.
    fn dependents(&self, issue: &Issue) -> Value {
        let uid = issue.uid().to_string();
        let mut dependents = Vec::new();
        for other in self.issues() {
            let mut edges: Vec<&(String, String)> = other.deps().keys().collect();
            edges.sort();
            for (_, kind) in edges.into_iter().filter(|(to, _)| *to == uid) {
                let mut dep = Object::new();
                dep.insert("id".into(), Value::from(other.key()));
                dep.insert("title".into(), field(other, Field::Title));
                dep.insert("status".into(), field(other, Field::Status));
                dep.insert("dependency_type".into(), Value::from(kind.as_str()));
                dependents.push(Value::Object(dep));
            }
        }
        dependents.into()
    }

    /// An issue on one line, as `list` prints it before escaping (Python's `line`).
    pub fn line(&self, issue: &Issue) -> String {
        let who = issue
            .get(Field::Assignee)
            .filter(|a| a.truthy())
            .map(|a| format!(" @{}", str_of(a)))
            .unwrap_or_default();
        let disputed = disputed(self, issue);
        let flag = if disputed.is_empty() {
            String::new()
        } else {
            format!(" ⚠ conflict: {}", disputed.join(", "))
        };
        let branch = branch(issue.where_())
            .map(|b| format!(" ({})", str_of(b)))
            .unwrap_or_default();
        format!(
            "{} {} [P{}] [{}] - {}{who}{branch}{flag}",
            icon(issue.status()),
            issue.key(),
            issue.priority(),
            str_of(&field(issue, Field::IssueType)),
            str_of(&field(issue, Field::Title)),
        )
    }
}

/// Strings as a JSON array.
fn texts<'a>(items: impl Iterator<Item = &'a str>) -> Value {
    Value::Array(items.map(Value::from).collect())
}

/// Imported dependencies on keys that are not issues yet, each key and type once, sorted: they bind
/// when the key arrives.
fn unresolved(issue: &Issue) -> Value {
    let unresolved: std::collections::BTreeSet<(String, String)> = issue
        .dangling()
        .iter()
        .map(|dep| {
            let key = dep.get("depends_on_id").map(str_of).unwrap_or_default();
            let kind = dep
                .get("type")
                .filter(|kind| kind.truthy())
                .map_or_else(|| "blocks".to_owned(), str_of);
            (key, kind)
        })
        .collect();
    let unresolved = unresolved.into_iter().map(|(key, kind)| {
        let mut dep = Object::new();
        dep.insert("id".into(), Value::from(key));
        dep.insert("dependency_type".into(), Value::from(kind));
        Value::Object(dep)
    });
    unresolved.collect::<Vec<_>>().into()
}

/// A comment as `show` lists it: its id (the imported one, or the event's), author, text, time and
/// branch.
fn shown_comment(comment: &Comment) -> Object {
    let members = comment.to_value();
    let get = |key: &str| members.get(key).cloned().unwrap_or(Value::Null);
    let legacy = get("legacy_id");
    let mut out = Object::new();
    out.insert(
        "id".into(),
        if legacy == Value::Null {
            get("uid")
        } else {
            legacy
        },
    );
    out.insert("author".into(), get("author"));
    out.insert("text".into(), get("text"));
    out.insert("created_at".into(), get("created_at"));
    if let Some(branch) = branch(members.get("where")) {
        out.insert("branch".into(), branch.clone());
    }
    out
}
