//! `bead create`: one issue, with its parent and dependencies, published at once.

use std::ffi::OsString;

use beadroll_core::code::Code;
use beadroll_core::json::{Object, Value};
use beadroll_core::query::blocking;
use beadroll_store::uuid7;
use beadroll_sync::{Tx, writing_spent};

use super::{
    ContentArgs, Context, Failure, Outcome, Payload, arg_text, arg_texts, encodable, field_args,
    need_tracker, resolve_arg, rpartition, split_args,
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
}

/// Creates an issue: its key minted under its parent or from its id, its parent and dependency
/// edges with it. In-progress is reached only through a claim.
pub(crate) fn create(ctx: &Context<'_>, args: &CreateArgs) -> Result<Outcome, Failure> {
    let title = [&args.title, &args.title_pos]
        .into_iter()
        .flatten()
        .find(|t| !t.is_empty());
    let Some(title) = title else {
        return Err(Failure::new(Code::Validation, "a title is required"));
    };
    let mut fields = field_args(args.title.as_deref(), &args.fields)?;
    fields.set_text("title", title);
    if fields.values.get("status") == Some(&Value::from("in_progress")) {
        return Err(Failure::new(
            Code::Validation,
            "create the issue, then claim it: in_progress is reached only through a claim",
        ));
    }
    let ((key, title, result), spent) =
        writing_spent(ctx.ws, ctx.actor, |tx| write_issue(tx, args, &fields))?;
    if args.silent {
        let mut out = Object::new();
        out.insert("id".into(), Value::from(key.as_str()));
        return Ok(Outcome::new(out, vec![key]).wrote_in(spent));
    }
    Ok(Outcome::new(result, vec![format!("✓ Created {key}: {title}")]).wrote_in(spent))
}

/// Emits the issue (`fields`, under `--parent`, depending on each `--deps` item, `[type:]KEY`,
/// with `--labels`): its key, its title and its detail.
fn write_issue(
    tx: &mut Tx<'_>,
    args: &CreateArgs,
    fields: &Payload,
) -> Result<(String, String, Object), Failure> {
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
                "blocks".to_owned()
            } else {
                kind
            };
            deps.push((resolve_arg(st, target)?.uid(), kind, valid));
        }
        if let Some(parent) = parent {
            for (target, kind, _) in &deps {
                if blocking(kind) && (*target == parent.uid() || st.reaches(*target, parent.uid()))
                {
                    return Err(Failure::new(
                        Code::Cycle,
                        format!(
                            "depending on {} would make {} wait for itself",
                            st.key_of(&target.to_string()),
                            parent.key()
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
    data.insert(
        "labels".into(),
        Value::Array(labels.into_iter().map(Value::from).collect()),
    );
    tx.emit("create", None, data, Some(uid.clone()))?;
    encodable(fields.valid && labels_valid)?;
    if let Some(parent) = &parent_uid {
        let mut edge = Object::new();
        edge.insert("to".into(), Value::from(parent.as_str()));
        edge.insert("type".into(), Value::from("parent-child"));
        tx.emit("dep_add", Some(&uid), edge, None)?;
    }
    for (target, kind, valid) in deps {
        let mut edge = Object::new();
        edge.insert("to".into(), Value::from(target.to_string()));
        edge.insert("type".into(), Value::from(kind));
        tx.emit("dep_add", Some(&uid), edge, None)?;
        encodable(valid)?;
    }
    let st = tx.state()?;
    let issue = st
        .issue_named(&uid)
        .ok_or_else(|| Failure::new(Code::Validation, "the created issue is not there"))?;
    Ok((
        issue.key().to_owned(),
        issue.title().to_owned(),
        st.detail(issue),
    ))
}
