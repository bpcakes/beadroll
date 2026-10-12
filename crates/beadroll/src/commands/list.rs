//! `bead list` and `bead ready`.

use std::ffi::OsString;

use beadroll_core::event::Field;
use beadroll_core::json::{Int, Object, Value};
use beadroll_store::view_outline;

use super::{
    Context, Failure, Filters, Outcome, first, need_tracker, resolve, select, text, texts,
};

/// The filters `list` and `ready` take, as given.
pub(crate) struct FilterArgs {
    pub(crate) issue_type: Vec<OsString>,
    pub(crate) label: Vec<OsString>,
    pub(crate) priority: Option<OsString>,
    pub(crate) assignee: Option<OsString>,
    pub(crate) limit: Int,
}

/// `list`'s arguments.
pub(crate) struct ListArgs {
    pub(crate) status: Vec<OsString>,
    pub(crate) all: bool,
    pub(crate) deferred: bool,
    pub(crate) title_contains: Option<OsString>,
    pub(crate) id: Vec<OsString>,
    pub(crate) parent: Option<OsString>,
    pub(crate) filters: FilterArgs,
}

/// `ready`'s arguments: one scope at most (the parser refuses more).
pub(crate) struct ReadyArgs {
    pub(crate) root: Option<OsString>,
    pub(crate) epic: Option<OsString>,
    pub(crate) parent: Option<OsString>,
    pub(crate) filters: FilterArgs,
}

fn optional(value: Option<&OsString>) -> Result<Option<String>, Failure> {
    value.map(|v| text(v)).transpose()
}

/// Issues (open ones by default), by priority, creation time and key; `--limit 0` lists them all.
pub(crate) fn list(ctx: &Context<'_>, args: &ListArgs) -> Result<Outcome, Failure> {
    let st = view_outline(ctx.ws)?;
    need_tracker(&st)?;
    let filters = Filters {
        status: texts(&args.status)?,
        all: args.all,
        deferred: args.deferred,
        issue_type: texts(&args.filters.issue_type)?,
        label: texts(&args.filters.label)?,
        priority: optional(args.filters.priority.as_ref())?,
        assignee: optional(args.filters.assignee.as_ref())?,
        title_contains: optional(args.title_contains.as_ref())?,
        id: texts(&args.id)?,
        parent: optional(args.parent.as_ref())?,
    };
    let items = select(&st, &filters)?;
    let total = items.len();
    let limit = &args.filters.limit;
    let shown = if *limit == Int::from(0) {
        items
    } else {
        first(items, limit)
    };
    let mut result = Object::new();
    result.insert(
        "issues".into(),
        Value::Array(shown.iter().map(|i| Value::from(st.brief(i))).collect()),
    );
    result.insert("total".into(), Value::from(total as i64));
    result.insert("limit".into(), Value::Int(limit.clone()));
    result.insert("offset".into(), Value::from(0));
    result.insert("has_more".into(), Value::Bool(shown.len() < total));
    let mut lines: Vec<String> = shown.iter().map(|i| st.line(i)).collect();
    if lines.is_empty() {
        lines.push("No issues".to_owned());
    }
    Ok(Outcome::new(result, lines))
}

/// Open issues nobody holds, nothing blocks and whose status is not disputed; with a scope, only
/// those beneath it. The scope decides membership only: readiness is judged on the whole tracker,
/// so a blocker outside it (on the root, an ancestor or a second parent) still holds an issue
/// inside it.
pub(crate) fn ready(ctx: &Context<'_>, args: &ReadyArgs) -> Result<Outcome, Failure> {
    let st = view_outline(ctx.ws)?;
    need_tracker(&st)?;
    let root = match &args.root {
        Some(root) => Some(text(root)?),
        None => optional(args.epic.as_ref())?,
    };
    let beneath = match &root {
        Some(root) => Some(st.descendants(resolve(&st, root)?.uid())),
        None => None,
    };
    let filters = Filters {
        status: vec!["open".to_owned()],
        issue_type: texts(&args.filters.issue_type)?,
        label: texts(&args.filters.label)?,
        priority: optional(args.filters.priority.as_ref())?,
        assignee: optional(args.filters.assignee.as_ref())?,
        parent: optional(args.parent.as_ref())?,
        ..Filters::default()
    };
    let mut items: Vec<_> = select(&st, &filters)?
        .into_iter()
        .filter(|i| beneath.as_ref().is_none_or(|b| b.contains(&i.uid())))
        .filter(|i| i.claim().is_none() && !st.is_blocked(i) && !st.disputed(i, Field::Status))
        .collect();
    if args.filters.limit != Int::from(0) {
        items = first(items, &args.filters.limit);
    }
    let result = Value::Array(items.iter().map(|i| Value::from(st.brief(i))).collect());
    let mut lines: Vec<String> = items.iter().map(|i| st.line(i)).collect();
    if lines.is_empty() {
        lines.push("Nothing is ready".to_owned());
    }
    Ok(Outcome::new(result, lines))
}
