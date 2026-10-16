//! `bead close`, `bead reopen` and `bead delete`: another owner's claim requires `--force`;
//! closing or deleting your own claim ends exactly that claim and publishes immediately. An issue
//! already as asked is left alone and said to be. `close --recursive` closes every issue beneath
//! each one too, children first, in the same transaction.

use std::collections::HashSet;
use std::ffi::OsString;

use beadroll_core::code::Code;
use beadroll_core::event::Field;
use beadroll_core::id::EventId;
use beadroll_core::json::{Object, Value};
use beadroll_core::state::{Issue, State};
use beadroll_core::text::str_of;
use beadroll_store::{Label, view_outline};
use beadroll_sync::{publish_soon, writing_spent};

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

/// The issues a transition answers for, in the order it writes them, and the issues beneath them
/// a recursive close leaves as they are.
struct Plan {
    issues: Vec<EventId>,
    left: Vec<EventId>,
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
    let written = writing_spent(ctx.ws, ctx.actor, |tx| {
        need_tracker(tx.state()?)?;
        let me = ctx.actor.label(&tx.replica);
        let st = tx.state()?;
        let mut plan = plan(st, args)?;
        if args.recursive && !args.force {
            // the issues left as they are too: one deleted without seeing a claim still holds it
            let subtree: Vec<EventId> = plan.issues.iter().chain(&plan.left).copied().collect();
            refuse_claimed(st, &subtree, &me)?;
        }
        let ends_claims = matches!(op, "close" | "delete");
        let mut decided = Decided {
            // a repeat of this actor's close that ended a claim still publishes it, as a plain
            // close does, though the issue beneath is left out of the answer; nothing is written
            // to those
            ended: ends_claims
                && (plan.left.iter())
                    .filter_map(|uid| st.issue(*uid))
                    .any(|issue| ended_by(issue, &me)),
            ..Decided::default()
        };
        // Each issue is decided from the state the events before it leave. An event changes only
        // its own issue, so until an issue is named again that is the state read before writing;
        // only then are the events decided so far written and the state read again
        // (roll-lqjj.28). Read before every issue, it replayed the journal after each event the
        // projection could not take.
        // the events decided and not yet written
        let mut events: Vec<(EventId, Object)> = Vec::new();
        // the issues of those events
        let mut written: HashSet<EventId> = HashSet::new();
        let mut failure = None;
        for &uid in &plan.issues {
            if written.contains(&uid) {
                for (uid, data) in events.drain(..) {
                    tx.emit(op, Some(&uid.to_string()), data, None)?;
                }
                written.clear();
            }
            match decide(tx.state()?, uid, (op, status), args, &me, &mut decided) {
                Ok(Some(data)) => {
                    written.insert(uid);
                    events.push((uid, data));
                }
                Ok(None) => {}
                Err(stop) => {
                    failure = Some(stop);
                    break;
                }
            }
        }
        for (uid, data) in events {
            tx.emit(op, Some(&uid.to_string()), data, None)?;
        }
        if let Some(failure) = failure.or_else(|| plan.unknown.take()) {
            return Err(failure);
        }
        let result = briefs(tx.state()?, &plan.issues)?;
        let Decided {
            ended,
            named,
            unchanged,
        } = decided;
        Ok::<_, Failure>((result, ended, named, unchanged, plan))
    })?;
    let ((mut result, ended, named, unchanged, plan), spent) = written;
    // An ended claim is published here, and its answer reads the store again: the transaction is
    // let go of before either, as the command line lets go of it before a publish (roll-lqjj.42).
    // Nothing but this order keeps the two apart, which the types do not: `xtask/tests/heap`
    // peaks a close or delete that ends a claim against the claim, and fails when it is a whole
    // projection above. A `--force --offline` close of another owner's claim neither publishes nor
    // reads (no claim of its own to name), and frees it all the same, which is harmless.
    let spent = (!ended).then_some(spent);
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
    if !plan.left.is_empty() {
        // after the issues' lines, before the notes
        lines.insert(result.len(), Line::from(left_as_they_are(plan.left.len())));
    }
    let result = Value::Array(result.into_iter().map(Value::from).collect());
    let outcome = Outcome::new(result, lines);
    Ok(match spent {
        Some(spent) => outcome.wrote_in(spent),
        None => outcome,
    })
}

/// What a transition's events name and leave: the claim each issue's event names (it ends exactly
/// that claim, never a successor's), whether a claim ended, and which issues it leaves unchanged.
#[derive(Default)]
struct Decided {
    named: Vec<(EventId, EventId)>,
    ended: bool,
    unchanged: Vec<bool>,
}

/// What `op`, giving `status`, writes for the issue `uid`, decided from `st`, the state the events
/// before it leave: the event's data, or nothing for an issue already as asked, which `decided`
/// marks unchanged; the claim the event ends, and whether one ended, go in `decided` too.
fn decide(
    st: &State,
    uid: EventId,
    (op, status): (&str, &str),
    args: &TransitionArgs,
    me: &Label,
    decided: &mut Decided,
) -> Result<Option<Object>, Failure> {
    let ends_claims = matches!(op, "close" | "delete");
    let issue = known(st, uid)?;
    let held = issue.claim();
    if let Some(held) = held
        && !me.is(&held.owner)
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
    let mut data = Object::new();
    if let Some(reason) = args.reason.as_deref().filter(|r| !r.is_empty()) {
        data.insert("reason".into(), Value::from(text(reason)?));
    }
    if ends_claims && let Some(held) = held.filter(|held| me.is(&held.owner)) {
        data.insert("claim".into(), Value::from(held.id.to_string()));
        match decided.named.iter_mut().find(|(issue, _)| *issue == uid) {
            Some(entry) => entry.1 = held.id,
            None => decided.named.push((uid, held.id)),
        }
    }
    decided.ended = decided.ended || held.is_some() || (ends_claims && ended_by(issue, me));
    let settled = already(st, issue, status);
    decided.unchanged.push(settled);
    Ok((!settled).then_some(data))
}

/// What `args` acts on in `st`: each id named, in the order given; with `--recursive`, each named
/// issue and every issue beneath it, every one after those beneath it, less the issues beneath
/// that are deleted or already closed with nothing to end. A deleted one is left so, where
/// closing it by name brings it back.
fn plan(st: &State, args: &TransitionArgs) -> Result<Plan, Failure> {
    let mut plan = Plan {
        issues: Vec::new(),
        left: Vec::new(),
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
            plan.left.push(uid);
        } else {
            plan.issues.push(uid);
        }
    }
    Ok(plan)
}

/// Refuses a recursive close of a subtree in which another owner has claimed issues, naming every
/// one, so that a single `takeover` round or `--force` settles them all.
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
