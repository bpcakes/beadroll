//! `bead blocked`, `bead search` and `bead lint`: questions about the whole tracker.

use std::ffi::OsStr;

use beadroll_core::event::{DONE, Field};
use beadroll_core::json::{Object, Value};
use beadroll_core::state::Issue;
use beadroll_core::text::str_of;
use beadroll_store::{view, view_outline};

use super::{Context, Failure, Line, Outcome, Tone, need_tracker, row};

/// Open issues something holds back, by key, each with the issues blocking it: every issue's
/// blockers are listed once for all of them (`State::blocker_lists`, roll-lqjj.24), so that a deep
/// tree is not walked up once for each issue in it.
pub(crate) fn blocked(ctx: &Context<'_>) -> Result<Outcome, Failure> {
    let st = view_outline(ctx.ws)?;
    need_tracker(&st)?;
    let lists = st.blocker_lists();
    let mut items: Vec<(&Issue, Vec<&Issue>)> = (st.issues())
        .filter(|i| !DONE.contains(&i.status()))
        .map(|i| (i, lists.of(i)))
        .filter(|(_, blockers)| !blockers.is_empty())
        .collect();
    items.sort_by(|(a, _), (b, _)| a.key().cmp(b.key()));
    let mut result = Vec::new();
    let mut lines: Vec<Line> = Vec::new();
    for (issue, blockers) in items {
        let blockers: Vec<String> = blockers.iter().map(|b| b.key().to_owned()).collect();
        lines.push(
            row(&st, issue)
                .with(Tone::Plain, "  ⟵ ")
                .with(Tone::Id, blockers.join(", ")),
        );
        let mut item = st.brief(issue);
        item.insert(
            "blocked_by".into(),
            Value::Array(blockers.into_iter().map(Value::from).collect()),
        );
        result.push(Value::from(item));
    }
    if lines.is_empty() {
        lines.push("Nothing is blocked".into());
    }
    Ok(Outcome::new(Value::Array(result), lines).answered_from(st))
}

/// Issues whose key, title, texts or comments contain `query`, ignoring case; open issues unless
/// `all` is set. A query that is not UTF-8 matches nothing.
pub(crate) fn search(ctx: &Context<'_>, query: &OsStr, all: bool) -> Result<Outcome, Failure> {
    let st = view(ctx.ws)?;
    need_tracker(&st)?;
    let needle = query.to_str().map(str::to_lowercase);
    let mut hits: Vec<&Issue> = Vec::new();
    for issue in st.issues() {
        if DONE.contains(&issue.status()) && !all {
            continue;
        }
        let Some(needle) = &needle else {
            continue;
        };
        let mut haystack: Vec<String> = vec![issue.key().to_owned(), issue.title().to_owned()];
        for field in [
            Field::Description,
            Field::AcceptanceCriteria,
            Field::Notes,
            Field::Design,
            Field::ExternalRef,
        ] {
            let text = issue
                .get(field)
                .filter(|v| v.truthy())
                .map(str_of)
                .unwrap_or_default();
            haystack.push(text);
        }
        for comment in issue.comments() {
            haystack.push(comment.get("text").map(|t| str_of(&t)).unwrap_or_default());
        }
        if haystack
            .iter()
            .any(|text| text.to_lowercase().contains(needle.as_str()))
        {
            hits.push(issue);
        }
    }
    hits.sort_by(|a, b| {
        a.priority()
            .cmp(&b.priority())
            .then_with(|| a.key().cmp(b.key()))
    });
    let result = Value::Array(hits.iter().map(|i| Value::from(st.brief(i))).collect());
    let mut lines: Vec<Line> = hits.iter().map(|i| row(&st, i)).collect();
    if lines.is_empty() {
        lines.push("No matches".into());
    }
    Ok(Outcome::new(result, lines).answered_from(st))
}

/// Open tasks, bugs and features without acceptance criteria, by key.
pub(crate) fn lint(ctx: &Context<'_>) -> Result<Outcome, Failure> {
    let st = view(ctx.ws)?;
    need_tracker(&st)?;
    let mut missing: Vec<&Issue> = st
        .issues()
        .filter(|i| {
            !DONE.contains(&i.status())
                && matches!(i.issue_type(), "task" | "bug" | "feature")
                && !i.get(Field::AcceptanceCriteria).is_some_and(Value::truthy)
        })
        .collect();
    missing.sort_by(|a, b| a.key().cmp(b.key()));
    let problem = "missing acceptance criteria";
    let findings: Vec<Value> = missing
        .iter()
        .map(|issue| {
            let mut item = Object::new();
            item.insert("id".into(), Value::from(issue.key()));
            item.insert(
                "title".into(),
                issue.get(Field::Title).cloned().unwrap_or(Value::Null),
            );
            item.insert("problem".into(), Value::from(problem));
            Value::from(item)
        })
        .collect();
    let mut lines: Vec<String> = missing
        .iter()
        .map(|issue| format!("{}: {problem}", issue.key()))
        .collect();
    if lines.is_empty() {
        lines.push("Clean".to_owned());
    }
    let mut result = Object::new();
    result.insert("count".into(), Value::from(findings.len() as i64));
    result.shift_insert(0, "findings".into(), Value::Array(findings));
    Ok(Outcome::new(result, lines).answered_from(st))
}
