//! `bead create`: one issue, with its parent and dependencies, published at once.

use std::ffi::OsString;

use beadroll_core::code::Code;
use beadroll_core::event::{BLOCKS, PARENT_CHILD, status};
use beadroll_core::json::{Object, Value};
use beadroll_core::op::OpName;
use beadroll_core::query::blocking;
use beadroll_core::state::{Issue, State};

use beadroll_store::{uuid7, view_outline};
use beadroll_sync::{NetworkMode, Tx, publish_soon, writing_spent};

use super::{
    ContentArgs, Context, Failure, Line, Outcome, Payload, Tone, arg_text, arg_texts, edge_data,
    encodable, field_args, holds_nothing, need_tracker, resolve_arg, rpartition, split_args,
    texts_in,
};

/// `create`'s arguments.
#[derive(Default)]
pub(crate) struct CreateArgs {
    pub(crate) title_pos: Option<OsString>,
    pub(crate) title: Option<OsString>,
    pub(crate) fields: ContentArgs,
    pub(crate) labels: Vec<OsString>,
    pub(crate) deps: Vec<OsString>,
    pub(crate) silent: bool,
    pub(crate) network: NetworkMode,
    pub(crate) remote: Option<OsString>,
}

/// Creates an issue: its key minted under its parent or from its id, its parent and dependency
/// edges with it. In-progress is reached only through a claim.
pub(crate) fn create(ctx: &Context<'_>, args: &CreateArgs) -> Result<Outcome, Failure> {
    // a title that holds nothing, spaces alone, is none, as `update` refuses one
    let title = [&args.title, &args.title_pos]
        .into_iter()
        .flatten()
        .find(|t| !holds_nothing(t));
    let Some(title) = title else {
        return Err(Failure::new(Code::Validation, "a title is required"));
    };
    let mut fields = field_args(args.title.as_deref(), &args.fields)?;
    fields.set_text("title", title);
    if fields.values.get("status") == Some(&Value::from(status::TOMBSTONE)) {
        return Err(Failure::new(
            Code::Validation,
            "an issue is tombstoned by `bead delete`, never created so",
        ));
    }
    if fields.values.get("status") == Some(&Value::from(status::IN_PROGRESS)) {
        return Err(Failure::new(
            Code::Validation,
            "create the issue, then claim it: in_progress is reached only through a claim",
        ));
    }
    let (((key, title, result), uid), spent) =
        writing_spent(ctx.ws, ctx.actor, |tx| write_issue(tx, args, &fields))?;
    let child = args.fields.parent.as_deref().is_some_and(|p| !p.is_empty());
    if !child {
        return Ok(answer(args, key, &title, result).wrote_in(spent));
    }
    // A child's key is the one a merge renumbers when another machine gave its parent a child of
    // the same number first: published here rather than after the answer, so that the answer
    // names the key the issue has once its publish has merged, never one that names another
    drop(spent);
    let warning = publish_soon(ctx.ws, args.network, args.remote.as_deref());
    let st = view_outline(ctx.ws)?;
    let eid = st.issue_named(&uid).map(Issue::uid);
    let st = texts_in(ctx.ws, st, eid)?;
    let (key, title, mut result) = shown(&st, &uid)?;
    if let Some(warning) = &warning {
        // --json is told too, as `update` tells it: the other machines do not know yet
        result.insert("unpublished".into(), Value::from(warning.as_str()));
    }
    let mut outcome = answer(args, key, &title, result);
    if let (Some(warning), Some(lines)) = (&warning, &mut outcome.lines) {
        lines.push(Line::toned(Tone::Warning, warning.clone()));
    }
    outcome
        .answered_from(st)
        .after_its_publish(ctx.ws, warning.as_deref())
}

/// What `create` answers of the issue it made, `key`: its key alone (`--silent`), or its detail.
fn answer(args: &CreateArgs, key: String, title: &str, result: Object) -> Outcome {
    if args.silent {
        let mut out = Object::new();
        out.insert("id".into(), Value::from(key.as_str()));
        return Outcome::new(out, vec![key]).first_line_alone();
    }
    Outcome::new(result, vec![format!("✓ Created {key}: {title}")])
}

/// The issue `uid` as `create` shows it: its key, its title and its detail.
fn shown(st: &State, uid: &str) -> Result<(String, String, Object), Failure> {
    let issue = st
        .issue_named(uid)
        .ok_or_else(|| Failure::new(Code::Validation, "the created issue is not there"))?;
    Ok((
        issue.key().to_owned(),
        issue.title().to_owned(),
        st.detail(issue.hydrated()?),
    ))
}

/// Emits the issue (`fields`, under `--parent`, depending on each `--deps` item, `[type:]KEY`,
/// with `--labels`): its key, its title and its detail, and its id.
fn write_issue(
    tx: &mut Tx<'_>,
    args: &CreateArgs,
    fields: &Payload,
) -> Result<((String, String, Object), String), Failure> {
    let clock = tx.ws.settings.clock.clone();
    let (deps, parent_uid, key, uid) = {
        let st = tx.state()?;
        need_tracker(st)?;
        let parent = match args.fields.parent.as_deref().filter(|p| !p.is_empty()) {
            Some(key) => Some(resolve_arg(st, key)?),
            None => None,
        };
        let mut deps = Vec::new();
        for spec in split_args(&args.deps) {
            let (kind, target) = rpartition(&spec, b':');
            let (kind, valid) = arg_text(kind);
            let kind = if kind.is_empty() {
                BLOCKS.to_owned()
            } else {
                kind
            };
            deps.push((resolve_arg(st, target)?.uid(), kind, valid));
        }
        // The new issue waits for what it blocks on, and each of its parents waits for it, whether
        // named by --parent or as a parent-child dependency: none of them may be reached from what
        // it waits for, or the edges would close a loop through it.
        let parents = (parent.map(|parent| parent.uid()).into_iter()).chain(
            (deps.iter())
                .filter(|(_, kind, _)| kind == PARENT_CHILD)
                .map(|(t, _, _)| *t),
        );
        for parent in parents {
            for (target, kind, _) in &deps {
                // reached through its parents alone, the new issue loops when a prerequisite waits on
                // one of them, what parents pass down counted
                if blocking(kind) && st.waits_on(*target, parent) {
                    return Err(Failure::new(
                        Code::Cycle,
                        format!(
                            "depending on {} would make {} wait for itself",
                            st.key_of(&target.to_string()),
                            st.key_of(&parent.to_string())
                        ),
                    ));
                }
            }
        }
        let uid = uuid7(&clock);
        let key = st.mint_key(&uid, parent)?;
        (deps, parent.map(|p| p.uid().to_string()), key, uid)
    };
    let (labels, labels_valid) = arg_texts(&split_args(&args.labels));
    let mut data = Object::new();
    data.insert("key".into(), Value::from(key));
    data.insert("fields".into(), Value::from(fields.values.clone()));
    data.insert("labels".into(), Value::from(labels));
    tx.emit(OpName::Create, None, data, Some(uid.clone()))?;
    encodable(fields.valid.and(labels_valid))?;
    if let Some(parent) = &parent_uid {
        tx.emit(
            OpName::DepAdd,
            Some(&uid),
            edge_data(parent, PARENT_CHILD),
            None,
        )?;
    }
    for (target, kind, valid) in deps {
        let edge = edge_data(&target.to_string(), &kind);
        tx.emit(OpName::DepAdd, Some(&uid), edge, None)?;
        encodable(valid)?;
    }
    let st = tx.state()?;
    Ok((shown(st, &uid)?, uid))
}
