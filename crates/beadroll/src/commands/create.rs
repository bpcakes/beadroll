//! `bead create`: one issue, with its parent and dependencies, published at once.

use std::ffi::OsString;

use beadroll_core::code::Code;
use beadroll_core::json::{Object, Value};
use beadroll_core::query::blocking;
use beadroll_store::uuid7;
use beadroll_sync::{Tx, writing};

use super::{
    Context, Failure, FieldFlags, Outcome, field_args, need_tracker, resolve, split_list, text,
    texts,
};

/// `create`'s arguments.
#[derive(Default)]
pub(crate) struct CreateArgs {
    pub(crate) title_pos: Option<OsString>,
    pub(crate) title: Option<OsString>,
    pub(crate) description: Option<OsString>,
    pub(crate) description_file: Option<OsString>,
    pub(crate) acceptance_criteria: Option<OsString>,
    pub(crate) notes: Option<OsString>,
    pub(crate) design: Option<OsString>,
    pub(crate) priority: Option<OsString>,
    pub(crate) issue_type: Option<OsString>,
    pub(crate) status: Option<OsString>,
    pub(crate) assignee: Option<OsString>,
    pub(crate) estimate: Option<beadroll_core::json::Int>,
    pub(crate) external_ref: Option<OsString>,
    pub(crate) parent: Option<OsString>,
    pub(crate) labels: Vec<OsString>,
    pub(crate) deps: Vec<OsString>,
    pub(crate) silent: bool,
    pub(crate) offline: bool,
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
    let title = text(title)?;
    let mut fields = field_args(&FieldFlags {
        title: args.title.as_deref(),
        description: args.description.as_deref(),
        description_file: args.description_file.as_deref(),
        acceptance_criteria: args.acceptance_criteria.as_deref(),
        notes: args.notes.as_deref(),
        design: args.design.as_deref(),
        assignee: args.assignee.as_deref(),
        external_ref: args.external_ref.as_deref(),
        priority: args.priority.as_deref(),
        issue_type: args.issue_type.as_deref(),
        estimate: args.estimate.clone(),
        status: args.status.as_deref(),
    })?;
    fields.insert("title".into(), Value::from(title));
    if fields.get("status") == Some(&Value::from("in_progress")) {
        return Err(Failure::new(
            Code::Validation,
            "create the issue, then claim it: in_progress is reached only through a claim",
        ));
    }
    let parent_key = args
        .parent
        .as_deref()
        .filter(|p| !p.is_empty())
        .map(text)
        .transpose()?;
    let specs = split_list(&texts(&args.deps)?);
    let labels = split_list(&texts(&args.labels)?);
    let (key, title, result) = writing(ctx.ws, ctx.actor, |tx| {
        write_issue(tx, fields, parent_key.as_deref(), &specs, &labels)
    })?;
    if args.silent {
        let mut out = Object::new();
        out.insert("id".into(), Value::from(key.as_str()));
        return Ok(Outcome::new(out, vec![key]));
    }
    Ok(Outcome::new(
        result,
        vec![format!("✓ Created {key}: {title}")],
    ))
}

/// Emits the issue (`fields`, under `parent_key`, depending on each of `specs`, `[type:]KEY`, with
/// `labels`): its key, its title and its detail.
fn write_issue(
    tx: &mut Tx<'_>,
    fields: Object,
    parent_key: Option<&str>,
    specs: &[String],
    labels: &[String],
) -> Result<(String, String, Object), Failure> {
    let clock = tx.ws.settings.clock;
    {
        let (deps, parent_uid, key, uid) = {
            let st = tx.state()?;
            need_tracker(st)?;
            let parent = match parent_key {
                Some(key) => Some(resolve(st, key)?),
                None => None,
            };
            let mut deps = Vec::new();
            for spec in specs {
                let (kind, target) = match spec.rfind(':') {
                    Some(at) => (&spec[..at], &spec[at + 1..]),
                    None => ("", spec.as_str()),
                };
                let kind = if kind.is_empty() { "blocks" } else { kind };
                deps.push((resolve(st, target)?.uid(), kind.to_owned()));
            }
            if let Some(parent) = parent {
                for (target, kind) in &deps {
                    if blocking(kind)
                        && (*target == parent.uid() || st.reaches(*target, parent.uid()))
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
        let mut data = Object::new();
        data.insert("key".into(), Value::from(key));
        data.insert("fields".into(), Value::Object(fields));
        data.insert(
            "labels".into(),
            Value::Array(labels.iter().map(|l| Value::from(l.as_str())).collect()),
        );
        tx.emit("create", None, data, Some(uid.clone()))?;
        if let Some(parent) = &parent_uid {
            let mut edge = Object::new();
            edge.insert("to".into(), Value::from(parent.as_str()));
            edge.insert("type".into(), Value::from("parent-child"));
            tx.emit("dep_add", Some(&uid), edge, None)?;
        }
        for (target, kind) in &deps {
            let mut edge = Object::new();
            edge.insert("to".into(), Value::from(target.to_string()));
            edge.insert("type".into(), Value::from(kind.as_str()));
            tx.emit("dep_add", Some(&uid), edge, None)?;
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
}
