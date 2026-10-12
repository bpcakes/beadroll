//! Ownership applied (Python's `apply_event` for `claim`, `release`, `close`, `reopen` and
//! `delete`, and `end_claim`): who holds an issue, and the claims that end, lose or come too late.

use std::sync::Arc;

use crate::event::Field;
use crate::id::EventId;
use crate::journal::Event;
use crate::json::Value;
use crate::op::{Claiming, Ending, PyInt};
use crate::state::{Claim, EndedClaim, Issue, LostClaim, Provisional};

use super::{Ctx, Held};

pub(super) fn claim(c: &Ctx<'_>, issue: &mut Issue, claiming: &Claiming) -> bool {
    let who = c.who;
    if claiming.mode.as_deref() == Some("offline") {
        // Provisional: never displaces a confirmed owner.
        if !c.claim.is_some_and(|held| held.mine) {
            issue.provisional_claims.retain(|p| *p.owner != *who);
            issue.provisional_claims.push(Provisional {
                id: c.eid,
                owner: c.name(who),
                where_: c.place.cloned(),
            });
            if c.claim.is_none() {
                c.write_status(issue, "in_progress", None, &[]);
            }
        }
    } else if c.claim.is_none() || takes_over(c.claim, claiming) {
        end_claim(c, issue, "takeover", claiming.reason.as_deref());
        // `max(generation + 1, data.get("generation") or 0)`: of two equal, the first.
        let next = issue.claim_generation.plus_one();
        let given = claiming
            .generation
            .as_ref()
            .filter(|g| !g.is_zero())
            .map(PyInt::number);
        issue.claim_generation = given.filter(|given| *given > next).unwrap_or(next);
        let mode = (claiming.mode.as_deref())
            .filter(|m| !m.is_empty())
            .unwrap_or("online");
        issue.claim = Some(Box::new(Claim {
            id: c.eid,
            owner: c.name(who),
            machine: Arc::clone(c.event.trace.shared_replica()),
            agent: actor_text(c.event.trace, "agent").map(|agent| c.name(agent)),
            thread: actor_text(c.event.trace, "thread").map(|thread| c.name(thread)),
            generation: issue.claim_generation.clone(),
            mode: c.name(mode),
            where_: c.place.cloned(),
        }));
        issue.provisional_claims.retain(|p| *p.owner != *who);
        c.write_status(issue, "in_progress", None, &[]);
        c.write(issue, Field::Assignee, Value::from(who), &[]);
    } else if c.claim.is_some_and(|held| !held.mine) {
        // Nothing has changed the issue's claim: it is still the one held before this event.
        let held_by =
            (issue.claim.as_ref()).map_or_else(|| c.name(""), |held| Arc::clone(&held.owner));
        issue.lost_claims.push(LostClaim {
            by: c.name(who),
            held_by,
            event: c.eid,
        });
        return false;
    }
    true
}

pub(super) fn release(c: &Ctx<'_>, issue: &mut Issue, ending: &Ending) -> bool {
    let named = ending.claim.as_deref();
    let named_id = named.and_then(EventId::parse);
    if named_id.is_some_and(|named| c.superseded.contains(&named)) {
        return c.stale(issue, named);
    }
    if let Some(held) = c.claim.filter(|held| Some(held.id) == named_id) {
        end_claim(c, issue, "release", ending.reason.as_deref());
        c.write_status(issue, "open", None, &[held.id]);
        c.write(issue, Field::Assignee, Value::Null, &[held.id]);
    } else if issue.provisional_claims.iter().any(|p| *p.owner == *c.who) {
        issue.provisional_claims.retain(|p| *p.owner != *c.who);
        if c.claim.is_none() && issue.provisional_claims.is_empty() {
            c.write_status(issue, "open", None, &[]);
        }
    } else {
        return c.stale(issue, named); // a delayed release from an owner that has since been replaced
    }
    true
}

/// `close` (`closing`) and `delete`: both may name the claim they end.
pub(super) fn close(c: &Ctx<'_>, issue: &mut Issue, ending: &Ending, closing: bool) -> bool {
    let named = ending.claim.as_deref().filter(|named| !named.is_empty());
    let named_id = named.and_then(EventId::parse);
    let current = c.claim.is_some_and(|held| Some(held.id) == named_id);
    if named.is_some() && (named_id.is_some_and(|named| c.superseded.contains(&named)) || !current)
    {
        return c.stale(issue, named); // an owner replaced by a takeover
    }
    let supersede: Vec<EventId> = named_id.into_iter().collect();
    let reason = ending.reason.as_deref();
    // A close that names its claim knows about it, even when the claim's bytes arrived later.
    c.write_status(
        issue,
        if closing { "closed" } else { "tombstone" },
        reason,
        &supersede,
    );
    if !closing {
        let extra = issue.extra_mut();
        extra.insert("deleted_at".into(), Value::from(c.when));
        extra.insert("deleted_by".into(), Value::from(c.who));
        extra.insert(
            "delete_reason".into(),
            Value::from(reason.unwrap_or_default()),
        );
    }
    if c.claim
        .is_some_and(|held| named.is_some() || c.k.saw_at(c.at, held.id))
    {
        if closing && named.is_some() {
            let assignee = issue.get(Field::Assignee).cloned().unwrap_or(Value::Null);
            c.write(issue, Field::Assignee, assignee, &supersede);
        }
        end_claim(c, issue, if closing { "close" } else { "delete" }, None);
    }
    if closing {
        issue.provisional_claims.retain(|p| !c.k.saw_at(c.at, p.id));
        if let Some(place) = c.place {
            issue.closed_on = Some(std::sync::Arc::clone(place));
        }
    }
    true
}

pub(super) fn reopen(c: &Ctx<'_>, issue: &mut Issue) -> bool {
    c.write_status(issue, "open", None, &[]);
    issue.closed_on = None;
    if c.claim.is_some_and(|held| c.k.saw_at(c.at, held.id)) {
        end_claim(c, issue, "reopen", None);
    }
    true
}

/// Whether a claim takes over the current one by naming it.
fn takes_over(claim: Option<Held>, claiming: &Claiming) -> bool {
    let named = claiming
        .takeover
        .as_deref()
        .filter(|t| !t.is_empty())
        .and_then(EventId::parse);
    claim.is_some_and(|held| named == Some(held.id))
}

/// A member of the event's actor, as the claim records it.
fn actor_text<'a>(trace: &'a Event, key: &str) -> Option<&'a str> {
    (trace.actor())
        .and_then(|actor| actor.get(key))
        .and_then(Value::as_str)
}

/// The issue's claim ended by this event, `how`, for `reason`.
fn end_claim(c: &Ctx<'_>, issue: &mut Issue, how: &'static str, reason: Option<&str>) {
    if let Some(claim) = issue.claim.take() {
        issue.claim_history.push(EndedClaim {
            claim: *claim,
            ending: c.eid,
            ended_by: c.name(c.who),
            how,
            reason: reason
                .filter(|reason| !reason.is_empty())
                .map(str::to_owned),
        });
    }
}
