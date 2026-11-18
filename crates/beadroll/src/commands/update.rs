//! `bead update`: fields, labels and parent, checked for every issue before anything is written.
//! Changing the status of an issue someone else holds needs `--force`; closing your own claim this
//! way ends it and is published at once. A claim (`--claim`, `--status in_progress`) and the edits
//! made with it are one attempt, coordinated with the remote: both land, or neither. An update
//! that asks for nothing writes nothing and says so.

use std::ffi::{OsStr, OsString};

use beadroll_core::code::Code;
use beadroll_core::event::{DONE, PARENT_CHILD, status};
use beadroll_core::id::EventId;
use beadroll_core::json::{Object, Value};
use beadroll_core::op::OpName;
use beadroll_core::state::{Issue, State};
use beadroll_core::text::str_of;
use beadroll_store::Label;
use beadroll_sync::{Emitter, Mode, NetworkMode, coordinated, publish_soon, writing_state};

use super::ownership::{ClaimStep, command_signature, details, still_held};
use super::{
    ContentArgs, Context, Failure, Line, Outcome, Payload, Tone, arg_texts, edge_data, encodable,
    ended_by, field_args, issue_line, need_tracker, refuse_foreign_claim, remove_edge, resolve_arg,
    split_args, view_of, view_of_written,
};

/// Whether an update requested any changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UpdateKind {
    /// A field, the status, a label or the parent was named.
    Changed,
    /// Nothing to update was named: every issue is answered unchanged.
    Empty,
}

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
    pub(crate) network: NetworkMode,
    pub(crate) remote: Option<OsString>,
    /// The parsed command line as the receipt names it (`Namespace::asked`).
    pub(crate) asked: String,
}

/// Changes each issue as asked; its detail afterwards.
pub(crate) fn update(ctx: &Context<'_>, args: &UpdateArgs) -> Result<Outcome, Failure> {
    let mut fields = field_args(args.title.as_deref(), &args.fields)?;
    let status = fields.values.shift_remove("status").map(|s| str_of(&s));
    if status.as_deref() == Some(status::TOMBSTONE) {
        return Err(Failure::new(
            Code::Validation,
            "use `bead delete` to tombstone an issue; it ends the claim as well",
        ));
    }
    // in_progress is reached only through a claim
    let wants_claim = args.claim || status.as_deref() == Some(status::IN_PROGRESS);
    let status = status.filter(|named| named != status::IN_PROGRESS);
    let kind = if fields.values.is_empty()
        && status.is_none()
        && !wants_claim
        && args.add_label.is_empty()
        && args.remove_label.is_empty()
        && args.set_labels.is_none()
        && args.fields.parent.as_deref().is_none_or(OsStr::is_empty)
    {
        UpdateKind::Empty
    } else {
        UpdateKind::Changed
    };
    let mut notes = Vec::new();
    // the state the write left, which the answer reads unless a publish follows (roll-lqjj.29)
    let mut written = None;
    // a retried claim may have been accepted and changed hands since
    let mut check_held = false;
    let ended = if wants_claim {
        let step = ClaimStep {
            actor: ctx.actor,
            keys: &args.ids,
            takeover: None,
        };
        let mut request = Object::new();
        request.insert("fields".into(), Value::from(fields.values.clone()));
        request.insert("status".into(), Value::from(status.clone()));
        let request = Value::from(request);
        let signature = command_signature(ctx.actor, &args.asked, Some(&request));
        let mut ended = Vec::new();
        // the claim and its edits are one attempt: both land, or neither
        let mode = coordinated(
            ctx.ws,
            ctx.actor,
            args.network,
            args.remote.as_deref(),
            &signature,
            |tx, mode| -> Result<Mode, Failure> {
                let mode = step.run(tx, mode)?;
                let me = ctx.actor.label(&tx.replica);
                let mut attempt = tx.attempt();
                ended.extend(edits(
                    attempt.lend()?,
                    &me,
                    args,
                    &fields,
                    status.as_deref(),
                )?);
                Ok(mode)
            },
        )?;
        if mode == Mode::Offline {
            notes.push("claim is PROVISIONAL: not exclusive until claimed online".to_owned());
        }
        // a claiming close ends the claim on purpose
        check_held = mode != Mode::Offline && status.as_deref() != Some(status::CLOSED);
        ended
    } else {
        let (ended, state) = writing_state(ctx.ws, ctx.actor, |tx| {
            let me = ctx.actor.label(&tx.replica);
            edits(tx.lend()?, &me, args, &fields, status.as_deref())
        })?;
        written = state;
        ended
    };
    // the same promptness `bead close` gives
    let warning = if ended.is_empty() {
        None
    } else {
        written = None; // the publish may bring in what other machines wrote: read after it
        publish_soon(ctx.ws, args.network, args.remote.as_deref())
    };
    let st = match written {
        Some(written) => view_of_written(ctx.ws, written, &args.ids)?,
        None => view_of(ctx.ws, &args.ids)?,
    };
    if check_held {
        still_held(ctx, &st, &args.ids)?;
    }
    let mut result = details(&st, &args.ids)?;
    let lines = answer(&mut result, kind, notes, warning.clone());
    let result = Value::from(result);
    let outcome = Outcome::new(result, lines).answered_from(st);
    outcome.after_its_publish(ctx.ws, warning.as_deref())
}

/// The answer's lines, `Updated` (or `Unchanged` when nothing was asked) for each issue, then
/// `notes` and `warning`, which `result` carries too, as warnings.
fn answer(
    result: &mut [Object],
    kind: UpdateKind,
    mut notes: Vec<String>,
    warning: Option<String>,
) -> Vec<Line> {
    if kind == UpdateKind::Empty {
        for item in result.iter_mut() {
            item.insert(
                "unchanged".into(),
                Value::from("nothing to update was given"),
            );
        }
        notes.push(
            "nothing to update was given: name a field, --status, a label or --parent \
             (bead update --help)"
                .to_owned(),
        );
    }
    let done = match kind {
        UpdateKind::Empty => "Unchanged",
        UpdateKind::Changed => "Updated",
    };
    let mut lines: Vec<Line> = result
        .iter()
        .map(|item| Line::from(issue_line(done, item)))
        .collect();
    lines.extend(
        notes
            .into_iter()
            .map(|note| Line::toned(Tone::Warning, note)),
    );
    if let Some(warning) = warning {
        for item in result.iter_mut() {
            // --json is told too: the other machines do not know yet
            item.insert("unpublished".into(), Value::from(warning.as_str()));
        }
        lines.push(Line::toned(Tone::Warning, warning));
    }
    lines
}

/// Validates every target first, then emits; nothing is written if any part is wrong. The keys of
/// the issues whose claim a close ends.
fn edits(
    (st, mut tx): (&State, Emitter<'_>),
    me: &Label,
    args: &UpdateArgs,
    fields: &Payload,
    status: Option<&str>,
) -> Result<Vec<String>, Failure> {
    need_tracker(st)?;
    let targets = args
        .ids
        .iter()
        .map(|key| resolve_arg(st, key))
        .collect::<Result<Vec<_>, Failure>>()?;
    let new_parent: Option<EventId> = match args.fields.parent.as_deref().filter(|p| !p.is_empty())
    {
        Some(key) => Some(resolve_arg(st, key)?.uid()),
        None => None,
    };
    let change = Change {
        args,
        fields,
        status,
        new_parent,
        me,
    };
    let mut ended = Vec::new();
    for issue in &targets {
        check(st, issue, &change)?;
        if status == Some(status::CLOSED) && (issue.claim().is_some() || ended_by(issue, me)) {
            ended.push(issue.key().to_owned());
        }
    }
    for issue in &targets {
        edit(&mut tx, st, issue, &change)?;
    }
    Ok(ended)
}

/// What an update asks of every issue it names, decided once from the command line and the state.
struct Change<'a> {
    /// The command line.
    args: &'a UpdateArgs,
    /// The fields to set, and whether their text can be written.
    fields: &'a Payload,
    /// The status asked for, if any (`in_progress` already made a claim).
    status: Option<&'a str>,
    /// The new parent, if `--parent` names one.
    new_parent: Option<EventId>,
    /// Who is asking.
    me: &'a Label,
}

/// Why `issue` cannot be changed as asked: a parent beneath it, someone else's claim on a status
/// change without `--force`, or your own claim given up by reopening rather than releasing.
fn check(st: &State, issue: &Issue, change: &Change<'_>) -> Result<(), Failure> {
    let Change {
        args,
        status,
        new_parent,
        me,
        ..
    } = *change;
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
    if status.is_some() && !args.force {
        refuse_foreign_claim(issue, me)?;
    }
    if status == Some(status::OPEN) && held.is_some_and(|held| me.is(&held.owner)) {
        return Err(Failure::new(
            Code::Validation,
            format!("{}: use `bead release` to give up your claim", issue.key()),
        ));
    }
    Ok(())
}

/// Emits the changes to one issue: its fields, its status, its labels and its parent.
fn edit(
    tx: &mut Emitter<'_>,
    st: &State,
    issue: &Issue,
    change: &Change<'_>,
) -> Result<(), Failure> {
    let Change {
        args,
        fields,
        status,
        new_parent,
        me,
    } = *change;
    let uid = issue.uid().to_string();
    if !fields.values.is_empty() {
        tx.emit(
            OpName::Set,
            Some(&uid),
            with("fields", fields.values.clone()),
            None,
        )?;
        encodable(fields.valid)?;
    }
    match status {
        Some(status::CLOSED) => {
            // the same transition `bead close` makes, ending exactly our claim
            let mut data = Object::new();
            if let Some(held) = issue.claim().filter(|held| me.is(&held.owner)) {
                data.insert("claim".into(), Value::from(held.id.to_string()));
            }
            tx.emit(OpName::Close, Some(&uid), data, None)?;
        }
        Some("open") if DONE.contains(&issue.status()) => {
            tx.emit(OpName::Reopen, Some(&uid), Object::new(), None)?;
        }
        Some(status) => {
            let mut changed = Object::new();
            changed.insert("status".into(), Value::from(status));
            tx.emit(OpName::Set, Some(&uid), with("fields", changed), None)?;
        }
        None => {}
    }
    for (op, given) in [
        (OpName::LabelAdd, &args.add_label),
        (OpName::LabelRemove, &args.remove_label),
    ] {
        if !given.is_empty() {
            labels(tx, op, &uid, &split_args(given))?;
        }
    }
    if let Some(set) = &args.set_labels {
        labels(
            tx,
            OpName::LabelSet,
            &uid,
            &split_args(std::slice::from_ref(set)),
        )?;
    }
    if let Some(parent) = new_parent {
        for edge in issue.deps().keys() {
            if edge.kind() == PARENT_CHILD {
                remove_edge(tx, st, issue, edge.to(), Some(edge.kind()))?;
            }
        }
        let edge = edge_data(&parent.to_string(), PARENT_CHILD);
        tx.emit(OpName::DepAdd, Some(&uid), edge, None)?;
    }
    Ok(())
}

/// `{name: value}`.
fn with(name: &str, value: Object) -> Object {
    let mut out = Object::new();
    out.insert(name.into(), Value::from(value));
    out
}

/// Emits a label event: `{"labels": [...]}`.
fn labels(tx: &mut Emitter<'_>, op: OpName, uid: &str, items: &[OsString]) -> Result<(), Failure> {
    let (items, valid) = arg_texts(items);
    let mut data = Object::new();
    data.insert("labels".into(), Value::from(items));
    tx.emit(op, Some(uid), data, None)?;
    encodable(valid)
}
