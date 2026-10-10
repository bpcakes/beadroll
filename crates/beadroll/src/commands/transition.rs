//! `bead close`, `bead reopen` and `bead delete` (Python's `transition`): someone else's claim needs
//! `--force`; closing or deleting your own claim ends exactly that claim, and is published at once.

use std::ffi::OsString;

use beadroll_core::code::Code;
use beadroll_core::id::EventId;
use beadroll_core::json::{Object, Value};
use beadroll_core::text::str_of;
use beadroll_store::view;
use beadroll_sync::{publish_soon, writing};

use super::{Context, Failure, Outcome, ended_by, need_tracker, resolve_arg, text};

/// The arguments `close`, `reopen` and `delete` take.
pub(crate) struct TransitionArgs {
    pub(crate) ids: Vec<OsString>,
    pub(crate) reason: Option<OsString>,
    pub(crate) force: bool,
    pub(crate) offline: bool,
    pub(crate) remote: Option<OsString>,
}

/// Applies `op` (`close`, `reopen` or `delete`) to each issue. When a claim ended, the result is
/// published at once; a close or delete that named a claim another owner had already taken over
/// is answered as `superseded`.
pub(crate) fn transition(
    ctx: &Context<'_>,
    op: &str,
    args: &TransitionArgs,
) -> Result<Outcome, Failure> {
    let verb = match op {
        "close" => "Closed",
        "reopen" => "Reopened",
        _ => "Deleted",
    };
    let ends_claims = matches!(op, "close" | "delete");
    let (mut result, ended, named) = writing(ctx.ws, ctx.actor, |tx| {
        need_tracker(tx.state()?)?;
        let me = ctx.actor.label(&tx.replica);
        let mut ended = false;
        // the claim each issue's event names: it ends exactly that claim, never a successor's
        let mut named: Vec<(EventId, EventId)> = Vec::new();
        for key in &args.ids {
            let (uid, shown, held, mine_ended) = {
                let st = tx.state()?;
                let issue = resolve_arg(st, key)?;
                let mine_ended = ended_by(issue, &me);
                (
                    issue.uid(),
                    issue.key().to_owned(),
                    issue.claim().cloned(),
                    mine_ended,
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
            tx.emit(op, Some(&uid.to_string()), data, None)?;
        }
        let st = tx.state()?;
        let result = args
            .ids
            .iter()
            .map(|key| Ok(st.brief(resolve_arg(st, key)?)))
            .collect::<Result<Vec<Object>, Failure>>()?;
        Ok::<_, Failure>((result, ended, named))
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
        superseded(ctx, op, args, &named, &mut result, &mut notes)?;
    }
    let mut lines: Vec<String> = result
        .iter()
        .map(|item| {
            let field = |name: &str| item.get(name).map(str_of).unwrap_or_default();
            format!("✓ {verb} {}: {}", field("id"), field("title"))
        })
        .collect();
    lines.extend(notes);
    let result = Value::Array(result.into_iter().map(Value::Object).collect());
    Ok(Outcome::new(result, lines))
}

/// After a close or delete that named claims was published: the issues whose claim another owner
/// had taken over before it reached the remote, marked `superseded`, with a note each.
fn superseded(
    ctx: &Context<'_>,
    op: &str,
    args: &TransitionArgs,
    named: &[(EventId, EventId)],
    result: &mut Vec<Object>,
    notes: &mut Vec<String>,
) -> Result<(), Failure> {
    let st = view(ctx.ws)?;
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
    *result = args
        .ids
        .iter()
        .map(|key| Ok(st.brief(resolve_arg(&st, key)?)))
        .collect::<Result<Vec<Object>, Failure>>()?;
    for item in result.iter_mut() {
        let key = item.get("id").map(str_of).unwrap_or_default();
        let uid = st.resolve(&key).map(|issue| issue.uid());
        if uid.is_some_and(|uid| lost.contains(&uid)) {
            let text = format!(
                "another owner took over before this {op} reached the remote; it is ignored"
            );
            item.insert("superseded".into(), Value::from(text.as_str()));
            notes.push(format!("! {key}: {text} (see `bead show`, under stale)"));
        }
    }
    Ok(())
}
