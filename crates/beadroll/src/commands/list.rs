//! `bead list` and `bead ready`.

use std::ffi::OsString;

use beadroll_core::event::Field;
use beadroll_core::json::{Int, Object, Value};
use beadroll_store::view_outline;

use super::args::{TextValidity, resolve_arg};
use super::{
    Context, Failure, Filters, Line, Outcome, first, need_tracker, row, select, split_args,
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

/// Filter values from the command line, split at commas: those that are UTF-8, and whether every
/// one given was (one that is not matches no issue).
fn values(given: &[OsString]) -> (Vec<String>, TextValidity) {
    let mut valid = TextValidity::Utf8;
    let kept = split_args(given)
        .into_iter()
        .filter_map(|value| {
            let text = value.into_string().ok();
            if text.is_none() {
                valid = TextValidity::Invalid;
            }
            text
        })
        .collect();
    (kept, valid)
}

/// The filters `list` and `ready` share from their arguments. A list of values any of which an
/// issue may have (statuses, types, ids) matches nothing when no value given is UTF-8; labels,
/// every one of which an issue must have, and the assignee and title match nothing when one is not.
fn filters(
    status: &[OsString],
    args: &FilterArgs,
    title_contains: Option<&OsString>,
    id: &[OsString],
) -> Filters {
    let any_of = |given: &[OsString]| {
        let (kept, valid) = values(given);
        let unmatched = valid == TextValidity::Invalid && kept.is_empty();
        (kept, unmatched)
    };
    let (status, no_status) = any_of(status);
    let (issue_type, no_type) = any_of(&args.issue_type);
    let (id, no_id) = any_of(id);
    let (label, labels_valid) = values(&args.label);
    let single = |value: Option<&OsString>| match value.map(|v| v.to_str()) {
        None => (None, false),
        Some(Some(text)) => (Some(text.to_owned()), false),
        Some(None) => (None, true),
    };
    let (assignee, no_assignee) = single(args.assignee.as_ref());
    let (title_contains, no_title) = single(title_contains);
    Filters {
        status,
        issue_type,
        label,
        priority: args.priority.clone(),
        assignee,
        title_contains,
        id,
        unmatched: no_status
            || no_type
            || no_id
            || labels_valid == TextValidity::Invalid
            || no_assignee
            || no_title,
        ..Filters::default()
    }
}

/// Issues (open ones by default), by priority, creation time and key; `--limit 0` lists them all.
pub(crate) fn list(ctx: &Context<'_>, args: &ListArgs) -> Result<Outcome, Failure> {
    let st = view_outline(ctx.ws)?;
    need_tracker(&st)?;
    let filters = Filters {
        all: args.all,
        deferred: args.deferred,
        parent: args.parent.clone(),
        ..filters(
            &args.status,
            &args.filters,
            args.title_contains.as_ref(),
            &args.id,
        )
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
    let mut lines: Vec<Line> = shown.iter().map(|i| row(&st, i)).collect();
    if lines.is_empty() {
        lines.push("No issues".into());
    }
    Ok(Outcome::new(result, lines).answered_from(st))
}

/// Open issues nobody holds, nothing blocks and whose status is not disputed; with a scope, only
/// those beneath it. The scope decides membership only: readiness is judged on the whole tracker,
/// so a blocker outside it (on the root, an ancestor or a second parent) still holds an issue
/// inside it, and once for every issue (`State::readiness`, roll-lqjj.19) rather than candidate by
/// candidate.
pub(crate) fn ready(ctx: &Context<'_>, args: &ReadyArgs) -> Result<Outcome, Failure> {
    let st = view_outline(ctx.ws)?;
    need_tracker(&st)?;
    let root = args.root.as_ref().or(args.epic.as_ref());
    let beneath = match root {
        Some(root) => Some(st.descendants(resolve_arg(&st, root)?.uid())),
        None => None,
    };
    let filters = Filters {
        status: vec!["open".to_owned()],
        parent: args.parent.clone(),
        ..filters(&[], &args.filters, None, &[])
    };
    let held = st.readiness();
    let mut items: Vec<_> = select(&st, &filters)?
        .into_iter()
        .filter(|i| beneath.as_ref().is_none_or(|b| b.contains(&i.uid())))
        .filter(|i| i.claim().is_none() && !held.is_blocked(i) && !st.disputed(i, Field::Status))
        .collect();
    if args.filters.limit != Int::from(0) {
        items = first(items, &args.filters.limit);
    }
    let result = Value::Array(items.iter().map(|i| Value::from(st.brief(i))).collect());
    let mut lines: Vec<Line> = items.iter().map(|i| row(&st, i)).collect();
    if lines.is_empty() {
        lines.push("Nothing is ready".into());
    }
    Ok(Outcome::new(result, lines).answered_from(st))
}
