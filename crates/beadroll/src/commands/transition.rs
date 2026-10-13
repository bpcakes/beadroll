//! `bead close`, `bead reopen` and `bead delete`: another owner's claim requires `--force`;
//! closing or deleting your own claim ends exactly that claim and publishes immediately. An issue
//! already as asked is left alone and said to be. `close --recursive` closes every issue beneath
//! each one too, children first, in the same transaction.

use std::ffi::OsString;

use beadroll_core::code::Code;
use beadroll_core::event::Field;
use beadroll_core::id::EventId;
use beadroll_core::json::{Object, Value};
use beadroll_core::state::{Issue, State};
use beadroll_core::text::str_of;
use beadroll_store::{Label, view_outline};
use beadroll_sync::{publish_soon, writing};

use super::{Context, Failure, Line, Outcome, Tone, ended_by, need_tracker, resolve_arg, text};

/// The arguments `close`, `reopen` and `delete` take.
pub(crate) struct TransitionArgs {
    pub(crate) ids: Vec<OsString>,
    pub(crate) reason: Option<OsString>,
    /// `close --recursive`: every issue beneath each one too.
    pub(crate) recursive: bool,
    pub(crate) force: bool,
    pub(crate) offline: bool,
    pub(crate) remote: Option<OsString>,
}

/// The issues a transition answers for, in the order it writes them, and how many issues beneath
/// them a recursive close leaves as they are.
struct Plan {
    issues: Vec<EventId>,
    left: usize,
    /// An id named without `--recursive` that names no issue: refused once the ids before it are,
    /// as each id was checked in turn before there was a plan.
    unknown: Option<Failure>,
}

/// Applies `op` (`close`, `reopen` or `delete`) to each issue, and with `--recursive` to every
/// issue beneath each, leaf-up. When a claim ended, the result is published at once; a close or
/// delete that named a claim another owner had already taken over is answered as `superseded`. An
/// issue named that is already in the status `op` gives, with no claim for it to end and that
/// status undisputed, gets no event: the answer marks it `unchanged`, and its reason stays the one
/// it has. An issue beneath in that state, or deleted, is left out.
pub(crate) fn transition(
    ctx: &Context<'_>,
    op: &str,
    args: &TransitionArgs,
) -> Result<Outcome, Failure> {
    // what it does, the status it gives, and that status as said
    let (verb, status, said) = match op {
        "close" => ("Closed", "closed", "closed"),
        "reopen" => ("Reopened", "open", "open"),
        _ => ("Deleted", "tombstone", "deleted"),
    };
    let ends_claims = matches!(op, "close" | "delete");
    let (mut result, ended, named, unchanged, plan) = writing(ctx.ws, ctx.actor, |tx| {
        need_tracker(tx.state()?)?;
        let me = ctx.actor.label(&tx.replica);
        let mut plan = plan(tx.state()?, args)?;
        if args.recursive && !args.force {
            refuse_claimed(tx.state()?, &plan.issues, &me)?;
        }
        let mut ended = false;
        // the claim each issue's event names: it ends exactly that claim, never a successor's
        let mut named: Vec<(EventId, EventId)> = Vec::new();
        let mut unchanged = Vec::new();
        for &uid in &plan.issues {
            let (shown, held, mine_ended, settled) = {
                let st = tx.state()?;
                let issue = known(st, uid)?;
                (
                    issue.key().to_owned(),
                    issue.claim().cloned(),
                    ended_by(issue, &me),
                    already(st, issue, status),
                )
            };
            if let Some(held) = &held
                && !me.is(&held.owner)
                && !args.force
            {
                return Err(Failure::new(
                    Code::ClaimedByOther,
                    format!(
                        "{shown} is claimed by {}: use `bead takeover` first, or --force",
                        held.owner
                    ),
                ));
            }
            let mut data = Object::new();
            if let Some(reason) = args.reason.as_deref().filter(|r| !r.is_empty()) {
                data.insert("reason".into(), Value::from(text(reason)?));
            }
            if ends_claims && let Some(held) = held.as_ref().filter(|held| me.is(&held.owner)) {
                data.insert("claim".into(), Value::from(held.id.to_string()));
                match named.iter_mut().find(|(issue, _)| *issue == uid) {
                    Some(entry) => entry.1 = held.id,
                    None => named.push((uid, held.id)),
                }
            }
            ended = ended || held.is_some() || (ends_claims && mine_ended);
            unchanged.push(settled);
            if !settled {
                tx.emit(op, Some(&uid.to_string()), data, None)?;
            }
        }
        if let Some(unknown) = plan.unknown.take() {
            return Err(unknown);
        }
        let st = tx.state()?;
        let result = briefs(st, &plan.issues)?;
        Ok::<_, Failure>((result, ended, named, unchanged, plan))
    })?;
    let warning = if ended {
        publish_soon(ctx.ws, args.offline, args.remote.as_deref())
    } else {
        None
    };
    let mut notes = Vec::new();
    if let Some(warning) = &warning {
        notes.push(warning.clone());
        for item in &mut result {
            // --json is told too: the other machines do not know yet
            item.insert("unpublished".into(), Value::from(warning.as_str()));
        }
    } else if !named.is_empty() {
        // published: answer with what the remote made of it
        superseded(ctx, op, &plan.issues, &named, &mut result, &mut notes)?;
    }
    let reason_given = args.reason.as_deref().is_some_and(|r| !r.is_empty());
    let mut lines = answer(&mut result, &unchanged, (verb, said), reason_given, notes);
    if plan.left > 0 {
        // after the issues' lines, before the notes
        lines.insert(result.len(), Line::from(left_as_they_are(plan.left)));
    }
    let result = Value::Array(result.into_iter().map(Value::from).collect());
    Ok(Outcome::new(result, lines))
}

/// What `args` acts on in `st`: each id named, in the order given; with `--recursive`, each named
/// issue and every issue beneath it, every one after those beneath it, less the issues beneath
/// that are deleted or already closed with nothing to end. A deleted one is left so, where
/// closing it by name brings it back.
fn plan(st: &State, args: &TransitionArgs) -> Result<Plan, Failure> {
    let mut plan = Plan {
        issues: Vec::new(),
        left: 0,
        unknown: None,
    };
    if !args.recursive {
        for key in &args.ids {
            match resolve_arg(st, key) {
                Ok(issue) => plan.issues.push(issue.uid()),
                Err(unknown) => {
                    plan.unknown = Some(unknown);
                    break;
                }
            }
        }
        return Ok(plan);
    }
    let named = args
        .ids
        .iter()
        .map(|key| resolve_arg(st, key).map(Issue::uid))
        .collect::<Result<Vec<EventId>, Failure>>()?;
    for uid in st.subtrees_leaf_up(&named) {
        let beneath = !named.contains(&uid);
        let done = st
            .issue(uid)
            .is_none_or(|issue| issue.status() == "tombstone" || already(st, issue, "closed"));
        if beneath && done {
            plan.left += 1;
        } else {
            plan.issues.push(uid);
        }
    }
    Ok(plan)
}

/// Refuses a recursive close of issues another owner has claimed, naming every one, so that a
/// single `takeover` round or `--force` settles them all.
fn refuse_claimed(st: &State, issues: &[EventId], me: &Label) -> Result<(), Failure> {
    let claimed: Vec<String> = issues
        .iter()
        .filter_map(|uid| st.issue(*uid))
        .filter_map(|issue| {
            let held = issue.claim().filter(|held| !me.is(&held.owner))?;
            Some(format!("{} is claimed by {}", issue.key(), held.owner))
        })
        .collect();
    if claimed.is_empty() {
        return Ok(());
    }
    Err(Failure::new(
        Code::ClaimedByOther,
        format!(
            "{}: use `bead takeover` first, or --force",
            claimed.join("; ")
        ),
    ))
}

/// The issue `uid`, which a plan made from this state found.
fn known(st: &State, uid: EventId) -> Result<&Issue, Failure> {
    st.issue(uid)
        .ok_or_else(|| Failure::new(Code::IssueNotFound, format!("Issue not found: {uid}")))
}

/// The briefs of `issues`, in that order.
fn briefs(st: &State, issues: &[EventId]) -> Result<Vec<Object>, Failure> {
    issues
        .iter()
        .map(|uid| Ok(st.brief(known(st, *uid)?)))
        .collect()
}

/// The line for `count` issues beneath that a recursive close left as they are.
fn left_as_they_are(count: usize) -> String {
    match count {
        1 => "1 issue beneath was already closed or deleted: left as it is".to_owned(),
        n => format!("{n} issues beneath were already closed or deleted: left as they are"),
    }
}

/// Whether another event giving `status` would change nothing on `issue`: it has that status, no
/// claim or provisional claim for the event to end, and no dispute over its status to settle.
fn already(st: &State, issue: &Issue, status: &str) -> bool {
    issue.status() == status
        && issue.claim().is_none()
        && issue.provisional_claims().is_empty()
        && !st.disputed_fields(issue).contains(&Field::Status)
}

/// The answer's lines, `verb` for each issue changed and "Already `said`" for each `unchanged`
/// one, which `result` marks too; then `notes`, and that a reason given for an unchanged issue is
/// not recorded.
fn answer(
    result: &mut [Object],
    unchanged: &[bool],
    (verb, said): (&str, &str),
    reason_given: bool,
    mut notes: Vec<String>,
) -> Vec<Line> {
    for (item, _) in result.iter_mut().zip(unchanged).filter(|(_, u)| **u) {
        item.insert("unchanged".into(), Value::from(format!("already {said}")));
        if reason_given {
            let key = item.get("id").map(str_of).unwrap_or_default();
            notes.push(format!(
                "{key} was already {said}: the reason given is not recorded"
            ));
        }
    }
    let mut lines: Vec<Line> = result
        .iter()
        .map(|item| {
            let field = |name: &str| item.get(name).map(str_of).unwrap_or_default();
            let done = match item.get("unchanged") {
                Some(_) => format!("Already {said}"),
                None => verb.to_owned(),
            };
            Line::from(format!("✓ {done} {}: {}", field("id"), field("title")))
        })
        .collect();
    lines.extend(
        notes
            .into_iter()
            .map(|note| Line::toned(Tone::Warning, note)),
    );
    lines
}

/// After a close or delete that named claims was published: the issues whose claim another owner
/// had taken over before it reached the remote, marked `superseded`, with a note each. `issues`
/// are those `result` answers for, in its order.
fn superseded(
    ctx: &Context<'_>,
    op: &str,
    issues: &[EventId],
    named: &[(EventId, EventId)],
    result: &mut Vec<Object>,
    notes: &mut Vec<String>,
) -> Result<(), Failure> {
    let st = view_outline(ctx.ws)?;
    let lost: Vec<EventId> = named
        .iter()
        .filter(|(uid, claim)| {
            st.issue(*uid).is_some_and(|issue| {
                issue.stale().iter().any(|stale| {
                    stale.op == op && stale.claim.as_deref() == Some(&claim.to_string())
                })
            })
        })
        .map(|(uid, _)| *uid)
        .collect();
    if lost.is_empty() {
        return Ok(());
    }
    *result = briefs(&st, issues)?;
    for (item, uid) in result.iter_mut().zip(issues) {
        if lost.contains(uid) {
            let key = item.get("id").map(str_of).unwrap_or_default();
            let text = format!(
                "another owner took over before this {op} reached the remote; it is ignored"
            );
            item.insert("superseded".into(), Value::from(text.as_str()));
            notes.push(format!("! {key}: {text} (see `bead show`, under stale)"));
        }
    }
    Ok(())
}
