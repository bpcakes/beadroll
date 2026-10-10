//! `bead update`: fields, labels and parent, checked for every issue before anything is written.
//! Changing the status of an issue someone else holds needs `--force`; closing your own claim this
//! way ends it and is published at once. A claim (`--claim`, `--status in_progress`) and the edits
//! made with it are one attempt, coordinated with the remote: both land, or neither.

use std::ffi::OsString;

use beadroll_core::code::Code;
use beadroll_core::event::DONE;
use beadroll_core::id::EventId;
use beadroll_core::json::{Object, Value};
use beadroll_core::state::{Issue, State, actor_label};
use beadroll_core::text::str_of;
use beadroll_store::view;
use beadroll_sync::{Mode, Tx, coordinated, publish_soon, writing};

use super::ownership::{ClaimStep, command_signature, still_held};
use super::{
    ContentArgs, Context, Failure, Outcome, Payload, arg_texts, encodable, ended_by, field_args,
    need_tracker, remove_edge, resolve_arg, split_args,
};

/// `update`'s arguments.
pub(crate) struct UpdateArgs {
    pub(crate) ids: Vec<OsString>,
    pub(crate) title: Option<OsString>,
    pub(crate) fields: ContentArgs,
    pub(crate) claim: bool,
    pub(crate) force: bool,
    pub(crate) add_label: Vec<OsString>,
    pub(crate) remove_label: Vec<OsString>,
    pub(crate) set_labels: Option<OsString>,
    pub(crate) offline: bool,
    pub(crate) remote: Option<OsString>,
    /// The parsed command line as the receipt names it (`Namespace::asked`).
    pub(crate) asked: String,
}

/// Changes each issue as asked; its detail afterwards.
pub(crate) fn update(ctx: &Context<'_>, args: &UpdateArgs) -> Result<Outcome, Failure> {
    let mut fields = field_args(args.title.as_deref(), &args.fields)?;
    let status = fields.values.shift_remove("status").map(|s| str_of(&s));
    if status.as_deref() == Some("tombstone") {
        return Err(Failure::new(
            Code::Validation,
            "use `bead delete` to tombstone an issue; it ends the claim as well",
        ));
    }
    // in_progress is reached only through a claim
    let wants_claim = args.claim || status.as_deref() == Some("in_progress");
    let status = status.filter(|status| status != "in_progress");
    let mut notes = Vec::new();
    let ended = if wants_claim {
        let step = ClaimStep {
            keys: &args.ids,
            takeover: None,
        };
        let mut request = Object::new();
        request.insert("fields".into(), Value::Object(fields.values.clone()));
        request.insert("status".into(), Value::from(status.clone()));
        let request = Value::Object(request);
        let signature = command_signature(ctx.actor, &args.asked, Some(&request));
        let mut ended = Vec::new();
        // the claim and its edits are one attempt: both land, or neither
        let mode = coordinated(
            ctx.ws,
            ctx.actor,
            args.offline,
            args.remote.as_deref(),
            &signature,
            |tx, mode| -> Result<Mode, Failure> {
                let mode = step.run(tx, mode)?;
                tx.conditional = true;
                ended.extend(edits(tx, args, &fields, status.as_deref())?);
                Ok(mode)
            },
        )?;
        if mode == Mode::Offline {
            notes.push("claim is PROVISIONAL: not exclusive until claimed online".to_owned());
        } else if status.as_deref() != Some("closed") {
            // a claiming close ends the claim on purpose
            still_held(ctx, &view(ctx.ws)?, &args.ids)?;
        }
        ended
    } else {
        writing(ctx.ws, ctx.actor, |tx| {
            edits(tx, args, &fields, status.as_deref())
        })?
    };
    // the same promptness `bead close` gives
    let warning = if ended.is_empty() {
        None
    } else {
        publish_soon(ctx.ws, args.offline, args.remote.as_deref())
    };
    let st = view(ctx.ws)?;
    let mut result = args
        .ids
        .iter()
        .map(|key| Ok(st.detail(resolve_arg(&st, key)?)))
        .collect::<Result<Vec<Object>, Failure>>()?;
    let mut lines: Vec<String> = result
        .iter()
        .map(|item| {
            let field = |name: &str| item.get(name).map(str_of).unwrap_or_default();
            format!("Updated {}: {}", field("id"), field("title"))
        })
        .collect();
    lines.extend(notes);
    if let Some(warning) = warning {
        for item in &mut result {
            // --json is told too: the other machines do not know yet
            item.insert("unpublished".into(), Value::from(warning.as_str()));
        }
        lines.push(warning);
    }
    let result = Value::Array(result.into_iter().map(Value::Object).collect());
    Ok(Outcome::new(result, lines))
}

/// Validates every target first, then emits; nothing is written if any part is wrong. The keys of
/// the issues whose claim a close ends.
fn edits(
    tx: &mut Tx<'_>,
    args: &UpdateArgs,
    fields: &Payload,
    status: Option<&str>,
) -> Result<Vec<String>, Failure> {
    let st: State = tx.state()?.clone();
    need_tracker(&st)?;
    let me = actor_label(Some(&tx.actor), &tx.replica);
    let targets = args
        .ids
        .iter()
        .map(|key| resolve_arg(&st, key))
        .collect::<Result<Vec<_>, Failure>>()?;
    let new_parent: Option<EventId> = match args.fields.parent.as_deref().filter(|p| !p.is_empty())
    {
        Some(key) => Some(resolve_arg(&st, key)?.uid()),
        None => None,
    };
    let mut ended = Vec::new();
    for issue in &targets {
        check(&st, issue, args, &me, status, new_parent)?;
        if status == Some("closed") && (issue.claim().is_some() || ended_by(issue, &me)) {
            ended.push(issue.key().to_owned());
        }
    }
    for issue in &targets {
        edit(tx, &st, issue, args, fields, status, new_parent, &me)?;
    }
    Ok(ended)
}

/// Why `issue` cannot be changed as asked: a parent beneath it, someone else's claim on a status
/// change without `--force`, or your own claim given up by reopening rather than releasing.
fn check(
    st: &State,
    issue: &Issue,
    args: &UpdateArgs,
    me: &str,
    status: Option<&str>,
    new_parent: Option<EventId>,
) -> Result<(), Failure> {
    let held = issue.claim();
    if let Some(parent) = new_parent
        && (parent == issue.uid() || st.reaches(issue.uid(), parent))
    {
        return Err(Failure::new(
            Code::Cycle,
            format!(
                "{} cannot become a child of {}: it would wait for itself",
                issue.key(),
                st.key_of(&parent.to_string())
            ),
        ));
    }
    if let (Some(_), Some(held)) = (status, held)
        && held.owner != me
        && !args.force
    {
        return Err(Failure::new(
            Code::ClaimedByOther,
            format!(
                "{} is claimed by {}: use `bead takeover` first, or --force",
                issue.key(),
                held.owner
            ),
        ));
    }
    if status == Some("open") && held.is_some_and(|held| held.owner == me) {
        return Err(Failure::new(
            Code::Validation,
            format!("{}: use `bead release` to give up your claim", issue.key()),
        ));
    }
    Ok(())
}

/// Emits the changes to one issue: its fields, its status, its labels and its parent.
#[allow(
    clippy::too_many_arguments,
    reason = "Python's edits closure reads all of these from its scope"
)]
fn edit(
    tx: &mut Tx<'_>,
    st: &State,
    issue: &Issue,
    args: &UpdateArgs,
    fields: &Payload,
    status: Option<&str>,
    new_parent: Option<EventId>,
    me: &str,
) -> Result<(), Failure> {
    let uid = issue.uid().to_string();
    if !fields.values.is_empty() {
        tx.emit(
            "set",
            Some(&uid),
            with("fields", fields.values.clone()),
            None,
        )?;
        encodable(fields.valid)?;
    }
    match status {
        Some("closed") => {
            // the same transition `bead close` makes, ending exactly our claim
            let mut data = Object::new();
            if let Some(held) = issue.claim().filter(|held| held.owner == me) {
                data.insert("claim".into(), Value::from(held.id.to_string()));
            }
            tx.emit("close", Some(&uid), data, None)?;
        }
        Some("open") if DONE.contains(&issue.status()) => {
            tx.emit("reopen", Some(&uid), Object::new(), None)?;
        }
        Some(status) => {
            let mut changed = Object::new();
            changed.insert("status".into(), Value::from(status));
            tx.emit("set", Some(&uid), with("fields", changed), None)?;
        }
        None => {}
    }
    for (op, given) in [
        ("label_add", &args.add_label),
        ("label_remove", &args.remove_label),
    ] {
        if !given.is_empty() {
            labels(tx, op, &uid, &split_args(given))?;
        }
    }
    if let Some(set) = &args.set_labels {
        labels(
            tx,
            "label_set",
            &uid,
            &split_args(std::slice::from_ref(set)),
        )?;
    }
    if let Some(parent) = new_parent {
        for (to, kind) in issue.deps().keys() {
            if kind == "parent-child" {
                remove_edge(tx, st, issue, to, Some(kind))?;
            }
        }
        let mut edge = Object::new();
        edge.insert("to".into(), Value::from(parent.to_string()));
        edge.insert("type".into(), Value::from("parent-child"));
        tx.emit("dep_add", Some(&uid), edge, None)?;
    }
    Ok(())
}

/// `{name: value}`.
fn with(name: &str, value: Object) -> Object {
    let mut out = Object::new();
    out.insert(name.into(), Value::Object(value));
    out
}

/// Emits a label event: `{"labels": [...]}`.
fn labels(tx: &mut Tx<'_>, op: &str, uid: &str, items: &[OsString]) -> Result<(), Failure> {
    let (items, valid) = arg_texts(items);
    let mut data = Object::new();
    data.insert(
        "labels".into(),
        Value::Array(items.into_iter().map(Value::from).collect()),
    );
    tx.emit(op, Some(uid), data, None)?;
    encodable(valid)
}
