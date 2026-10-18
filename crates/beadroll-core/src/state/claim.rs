//! An issue's ownership records: confirmed owner, ended claims, provisional claims, claims that
//! lost to a current owner and stale claim endings. The current claim, its generation and the
//! provisional claims are what every ownership decision reads; the ended claims, the lost claims
//! and the stale endings are the issue's ownership history, kept with its texts (roll-lqjj.40).

use std::sync::Arc;

use crate::id::EventId;
use crate::json::{Int, Object, Value};
use crate::time::event_time;

/// The confirmed owner of an issue. Its names are shared with every record that names the same,
/// and its time is its claim event's, which its id holds.
#[derive(Clone, Debug, PartialEq)]
pub struct Claim {
    /// The claim event.
    pub id: EventId,
    /// Who holds it.
    pub owner: Arc<str>,
    /// The machine that claimed it.
    pub machine: Arc<str>,
    /// The agent, as the claim's actor named it.
    pub agent: Option<Arc<str>>,
    /// The agent's thread.
    pub thread: Option<Arc<str>>,
    /// The issue's claim count when this claim took it.
    pub generation: Int,
    /// `online`, or how the claim says it was made.
    pub mode: Arc<str>,
    /// The checkout it was made in.
    pub where_: Option<Arc<Value>>,
}

impl Claim {
    /// When: its claim event's time.
    pub fn since(&self) -> String {
        event_time(self.id.millis())
    }

    /// As a JSON object, in field order.
    pub fn to_value(&self) -> Object {
        let mut out = Object::new();
        out.insert("id".into(), Value::from(self.id.to_string()));
        out.insert("owner".into(), Value::from(&*self.owner));
        out.insert("machine".into(), Value::from(&*self.machine));
        out.insert("agent".into(), Value::from(self.agent.as_deref()));
        out.insert("thread".into(), Value::from(self.thread.as_deref()));
        out.insert("generation".into(), Value::Int(self.generation.clone()));
        out.insert("since".into(), Value::from(self.since()));
        out.insert("mode".into(), Value::from(&*self.mode));
        if let Some(place) = &self.where_ {
            out.insert("where".into(), Value::clone(place));
        }
        out
    }
}

/// A claim that ended.
#[derive(Clone, Debug, PartialEq)]
pub struct EndedClaim {
    /// The claim.
    pub claim: Claim,
    /// The event that ended it, whose time is when.
    pub ending: EventId,
    /// Who ended it.
    pub ended_by: Arc<str>,
    /// How: `takeover`, `release`, `close`, `reopen` or `delete`.
    pub how: &'static str,
    /// The reason given, if any.
    pub reason: Option<String>,
}

impl EndedClaim {
    /// Who ended it, and how.
    pub fn ending(&self) -> Ended {
        Ended {
            by: Arc::clone(&self.ended_by),
            how: self.how,
        }
    }

    /// When it ended: the time of the event that ended it.
    pub fn ended(&self) -> String {
        event_time(self.ending.millis())
    }

    /// As a JSON object, in field order.
    pub fn to_value(&self) -> Object {
        let mut out = self.claim.to_value();
        out.insert("ended".into(), Value::from(self.ended()));
        out.insert("ended_by".into(), Value::from(&*self.ended_by));
        out.insert("how".into(), Value::from(self.how));
        if let Some(reason) = &self.reason {
            out.insert("reason".into(), Value::from(reason.as_str()));
        }
        out
    }
}

/// Who ended a claim, and how: what a command asks of an issue's last ended claim whether or not
/// the issue's texts, which hold its ownership history, are read in (roll-lqjj.40,
/// [`crate::state::Issue::last_ended`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ended {
    /// Who ended it.
    pub by: Arc<str>,
    /// How, as [`EndedClaim::how`] says.
    pub how: &'static str,
}

/// An offline claim.
#[derive(Clone, Debug, PartialEq)]
pub struct Provisional {
    /// The claim event.
    pub id: EventId,
    /// Who made it.
    pub owner: Arc<str>,
    /// The checkout it was made in.
    pub where_: Option<Arc<Value>>,
}

impl Provisional {
    /// When: its claim event's time.
    pub fn since(&self) -> String {
        event_time(self.id.millis())
    }

    /// As a JSON object, in field order.
    pub fn to_value(&self) -> Object {
        let mut out = Object::new();
        out.insert("id".into(), Value::from(self.id.to_string()));
        out.insert("owner".into(), Value::from(&*self.owner));
        out.insert("since".into(), Value::from(self.since()));
        if let Some(place) = &self.where_ {
            out.insert("where".into(), Value::clone(place));
        }
        out
    }
}

/// A claim that lost to the owner of the time.
#[derive(Clone, Debug, PartialEq)]
pub struct LostClaim {
    /// Who tried.
    pub by: Arc<str>,
    /// Who held the issue.
    pub held_by: Arc<str>,
    /// The claim event.
    pub event: EventId,
}

impl LostClaim {
    /// When: its claim event's time.
    pub fn at(&self) -> String {
        event_time(self.event.millis())
    }

    /// As a JSON object, in field order.
    pub fn to_value(&self) -> Object {
        let mut out = Object::new();
        out.insert("by".into(), Value::from(&*self.by));
        out.insert("at".into(), Value::from(self.at()));
        out.insert("held_by".into(), Value::from(&*self.held_by));
        out.insert("event".into(), Value::from(self.event.to_string()));
        out
    }
}

/// An end of a claim that was no longer current, ignored.
#[derive(Clone, Debug, PartialEq)]
pub struct Stale {
    /// `release`, `close` or `delete`.
    pub op: String,
    /// Who.
    pub by: Arc<str>,
    /// When.
    pub at: String,
    /// The claim it named.
    pub claim: Option<String>,
}

impl Stale {
    /// As a JSON object, in field order.
    pub fn to_value(&self) -> Object {
        let mut out = Object::new();
        out.insert("op".into(), Value::from(self.op.as_str()));
        out.insert("by".into(), Value::from(&*self.by));
        out.insert("at".into(), Value::from(self.at.as_str()));
        out.insert("claim".into(), Value::from(self.claim.clone()));
        out
    }
}
