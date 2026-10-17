//! Issue presentation: objects in stable field order and the one-line list format.

mod dependents;
mod details;

use crate::event::{EXPORT_FIELDS, Field};
use crate::json::{Object, Value};
use crate::query::Alternative;
use crate::state::{Comment, Edge, Issue, State};
use crate::text::str_of;

pub use dependents::Dependents;
pub use details::Details;

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

/// A field's value, or null when absent.
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
    let mut names: Vec<String> = (st.disputed_fields(issue).into_iter())
        .map(|field| field.name().to_owned())
        .collect();
    names.sort();
    names
}

impl State {
    /// An issue's list summary.
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

    /// Full issue details. The CLI adds the git-dependent `merged` field after `closed_on`. The
    /// issues that depend on it are found by reading every issue's edges, once, and whether
    /// anything holds it by walking its ancestors; a reader that shows several issues makes what
    /// their details share once for all of them with [`State::details_of`].
    pub fn detail(&self, issue: &Issue) -> Object {
        self.detail_with(issue, &self.dependents_of([issue]))
    }

    /// Full issue details as [`State::detail`] gives them, the issues that depend on it taken from
    /// `dependents` (roll-lqjj.20): found there when they were found in this state and it was asked
    /// about, and for it alone otherwise. Whether anything holds it is judged for it alone.
    pub fn detail_with(&self, issue: &Issue, dependents: &Dependents<'_>) -> Object {
        self.detail_judged(issue, dependents, self.is_blocked(issue))
    }

    /// Full issue details as [`State::detail_with`] gives them, `blocked` being whether anything
    /// holds the issue, as [`State::is_blocked`] answers.
    fn detail_judged(&self, issue: &Issue, dependents: &Dependents<'_>, blocked: bool) -> Object {
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
        let alone;
        let found = (dependents.found_in(self))
            .then(|| dependents.of(issue))
            .flatten();
        let dependents = match found {
            Some(found) => found,
            None => {
                alone = self.dependents_of([issue]);
                alone.of(issue).unwrap_or_default()
            }
        };
        out.insert("dependents".into(), dependents_shown(dependents));
        let comments = issue
            .comments()
            .iter()
            .map(|comment| Value::from(shown_comment(comment)));
        out.insert("comments".into(), comments.collect::<Vec<_>>().into());
        out.insert("blocked".into(), Value::from(blocked));
        out.insert("origin".into(), Value::from(issue.origin()));
        if let Some(place) = issue.where_().filter(|place| place.truthy()) {
            out.insert("where".into(), place.clone());
        }
        if let Some(place) = issue.closed_on().filter(|place| place.truthy()) {
            out.insert("closed_on".into(), place.clone());
        }
        if let Some(claim) = issue.claim() {
            out.insert("claim".into(), Value::from(claim.to_value()));
        }
        let objects = |items: Vec<Object>| items.into_iter().map(Value::from).collect::<Vec<_>>();
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
            out.insert(
                "conflicts".into(),
                Value::from(conflicts.collect::<Object>()),
            );
        }
        out
    }

    /// What an issue depends on, by sorted edge: the key, title and status of each target (an
    /// edge to text that names no issue shows it with status `unknown`).
    fn dependencies_shown(&self, issue: &Issue) -> Value {
        let mut edges: Vec<&Edge> = issue.deps().keys().collect();
        edges.sort();
        let dependencies = edges.iter().map(|edge| {
            let target = self.target(edge.to());
            let mut dep = Object::new();
            dep.insert("id".into(), Value::from(&*self.target_key(edge.to())));
            dep.insert("title".into(), Value::from(target.map_or("", Issue::title)));
            dep.insert(
                "status".into(),
                Value::from(target.map_or("unknown", Issue::status)),
            );
            dep.insert("dependency_type".into(), Value::from(edge.kind()));
            Value::from(dep)
        });
        dependencies.collect::<Vec<_>>().into()
    }

    /// The one-line issue summary before output escaping.
    pub fn line(&self, issue: &Issue) -> String {
        self.row(issue).to_string()
    }

    /// The one-line issue summary in parts, for a reader that shows them apart ([`Row`]).
    pub fn row(&self, issue: &Issue) -> Row {
        Row {
            icon: icon(issue.status()),
            status: issue.status().to_owned(),
            key: issue.key().to_owned(),
            priority: issue.priority().to_string(),
            kind: str_of(&field(issue, Field::IssueType)),
            title: str_of(&field(issue, Field::Title)),
            assignee: issue
                .get(Field::Assignee)
                .filter(|a| a.truthy())
                .map(str_of),
            branch: branch(issue.where_()).map(str_of),
            disputed: disputed(self, issue),
        }
    }
}

/// An issue's one-line summary in parts: `{icon} {key} [P{priority}] [{kind}] - {title}`, then
/// ` @{assignee}`, ` ({branch})` and ` ⚠ conflict: {disputed}` when there are any.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// The status icon ([`icon`]).
    pub icon: &'static str,
    /// The status.
    pub status: String,
    /// The key.
    pub key: String,
    /// The priority, as it prints.
    pub priority: String,
    /// The type.
    pub kind: String,
    /// The title.
    pub title: String,
    /// The assignee, when there is one.
    pub assignee: Option<String>,
    /// The branch it was last worked on, when one is named.
    pub branch: Option<String>,
    /// The names of its disputed fields, sorted.
    pub disputed: Vec<String>,
}

impl std::fmt::Display for Row {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Row {
            icon,
            key,
            priority,
            kind,
            title,
            ..
        } = self;
        write!(out, "{icon} {key} [P{priority}] [{kind}] - {title}")?;
        if let Some(who) = &self.assignee {
            write!(out, " @{who}")?;
        }
        if let Some(branch) = &self.branch {
            write!(out, " ({branch})")?;
        }
        if !self.disputed.is_empty() {
            write!(out, " ⚠ conflict: {}", self.disputed.join(", "))?;
        }
        Ok(())
    }
}

/// The issues that depend on one, as [`State::dependents_of`] found them: the key, title and status
/// of each, and the type of its edge.
fn dependents_shown(dependents: &[(&Issue, &str)]) -> Value {
    let dependents = dependents.iter().map(|(other, kind)| {
        let mut dep = Object::new();
        dep.insert("id".into(), Value::from(other.key()));
        dep.insert("title".into(), field(other, Field::Title));
        dep.insert("status".into(), field(other, Field::Status));
        dep.insert("dependency_type".into(), Value::from(*kind));
        Value::from(dep)
    });
    dependents.collect::<Vec<_>>().into()
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
        Value::from(dep)
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
