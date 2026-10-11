//! `bead claim`, `bead takeover` and `bead release`: ownership, coordinated with the remote
//! (`beadroll_sync::coordinated`): a claim is exclusive once the remote accepts it, and lasts until
//! a release, a close or a takeover.

use std::ffi::{OsStr, OsString};

use beadroll_core::code::Code;
use beadroll_core::event::Field;
use beadroll_core::json::{Format, Object, Value, write};
use beadroll_core::state::State;
use beadroll_core::text::str_of;
use beadroll_store::{Actor, replica_id, view};
use beadroll_sync::{Mode, Tx, coordinated};

use super::{Context, Failure, Outcome, arg_text, encodable, need_tracker, resolve_arg};

/// The arguments the coordinated commands share.
pub(crate) struct OwnershipArgs {
    pub(crate) ids: Vec<OsString>,
    pub(crate) reason: Option<OsString>,
    pub(crate) offline: bool,
    pub(crate) remote: Option<OsString>,
    /// The parsed command line as the receipt names it (`Namespace::asked`).
    pub(crate) asked: String,
}

/// What a claim says of how it was made (Python's `CLAIM_NOTE`).
fn claim_note(mode: Mode) -> &'static str {
    match mode {
        Mode::Online => "confirmed",
        Mode::Local => "local tracker, no remote",
        Mode::Offline => "PROVISIONAL: not exclusive until claimed online",
    }
}

/// The receipt of a coordinated command (Python's `command_signature`): `json.dumps({"actor": ...,
/// "args": ..., "request": ...}, sort_keys=True, default=str)`, byte for byte, so that either
/// implementation recognises the other's retry.
pub(crate) fn command_signature(actor: &Actor, asked: &str, request: Option<&Value>) -> String {
    let json = |value: &Value| write(value, Format::SORTED_ASCII).unwrap_or_default();
    let request = request.map_or_else(|| "null".to_owned(), json);
    format!(
        "{{\"actor\": {}, \"args\": {asked}, \"request\": {request}}}",
        json(&actor.value)
    )
}

/// The claim step of a coordinated transaction (Python's `claim_builder`): each issue `keys` names
/// claimed for the actor, generation one more than its last; with `takeover`, from its owner. A
/// done issue, a disputed status, or (without `takeover`) someone else's claim refuses all of it.
pub(crate) struct ClaimStep<'a> {
    pub(crate) actor: &'a Actor,
    pub(crate) keys: &'a [OsString],
    pub(crate) takeover: Option<&'a OsStr>,
}

impl ClaimStep<'_> {
    pub(crate) fn run(&self, tx: &mut Tx<'_>, mode: Mode) -> Result<Mode, Failure> {
        let st: State = tx.state()?.clone();
        need_tracker(&st)?;
        let me = self.actor.label(&tx.replica);
        for key in self.keys {
            let issue = resolve_arg(&st, key)?;
            let held = issue.claim();
            if st.is_done(issue) {
                return Err(Failure::new(
                    Code::Validation,
                    format!("{} is {}", issue.key(), issue.status()),
                ));
            }
            if st.disputed(issue, Field::Status) {
                return Err(Failure::new(
                    Code::StatusConflict,
                    format!(
                        "{} has a disputed status; settle it before claiming",
                        issue.key()
                    ),
                ));
            }
            if held.is_some_and(|held| me.is(&held.owner)) {
                continue;
            }
            if let Some(held) = held
                && self.takeover.is_none()
            {
                return Err(Failure::new(
                    Code::AlreadyClaimed,
                    format!(
                        "{} is claimed by {} (generation {}, since {})",
                        issue.key(),
                        held.owner,
                        held.generation,
                        held.since
                    ),
                ));
            }
            let mut data = Object::new();
            data.insert("mode".into(), Value::from(mode.as_str()));
            data.insert(
                "generation".into(),
                Value::Int(issue.claim_generation().plus_one()),
            );
            let mut valid = true;
            if let (Some(reason), Some(held)) = (self.takeover, held) {
                let (reason, ok) = arg_text(reason);
                valid = ok;
                data.insert("takeover".into(), Value::from(held.id.to_string()));
                data.insert("reason".into(), Value::from(reason));
            }
            tx.emit("claim", Some(&issue.uid().to_string()), data, None)?;
            encodable(valid)?;
        }
        Ok(mode)
    }
}

/// A retry recognised from its receipt answers for a claim accepted earlier, which may have moved
/// on since (Python's `still_held`): it says so rather than report a claim the actor no longer
/// holds.
pub(crate) fn still_held(ctx: &Context<'_>, st: &State, keys: &[OsString]) -> Result<(), Failure> {
    let replica = replica_id(&ctx.ws.settings, true)?.unwrap_or_default();
    let me = ctx.actor.label(&replica);
    for key in keys {
        let issue = resolve_arg(st, key)?;
        match issue.claim() {
            Some(held) if me.is(&held.owner) => {}
            Some(held) => {
                return Err(Failure::new(
                    Code::AlreadyClaimed,
                    format!(
                        "{}: your claim was accepted but is now held by {}",
                        issue.key(),
                        held.owner
                    ),
                ));
            }
            None => {
                return Err(Failure::new(
                    Code::NotOwner,
                    format!(
                        "{}: your claim was accepted but has since ended ({})",
                        issue.key(),
                        issue.status()
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// Each issue's detail, as `claim` and `takeover` answer with it.
fn details(st: &State, keys: &[OsString]) -> Result<Vec<Object>, Failure> {
    keys.iter()
        .map(|key| Ok(st.detail(resolve_arg(st, key)?)))
        .collect()
}

/// `bead claim`: takes ownership of each issue.
pub(crate) fn claim(ctx: &Context<'_>, args: &OwnershipArgs) -> Result<Outcome, Failure> {
    let step = ClaimStep {
        actor: ctx.actor,
        keys: &args.ids,
        takeover: None,
    };
    let signature = command_signature(ctx.actor, &args.asked, None);
    let mode = coordinated(
        ctx.ws,
        ctx.actor,
        args.offline,
        args.remote.as_deref(),
        &signature,
        |tx, mode| step.run(tx, mode),
    )?;
    let st = view(ctx.ws)?;
    if mode != Mode::Offline {
        still_held(ctx, &st, &args.ids)?;
    }
    let result = details(&st, &args.ids)?;
    let lines = result
        .iter()
        .map(|item| {
            let field = |name: &str| item.get(name).map(str_of).unwrap_or_default();
            format!(
                "Claimed {}: {} ({})",
                field("id"),
                field("title"),
                claim_note(mode)
            )
        })
        .collect();
    let result = Value::Array(result.into_iter().map(Value::from).collect());
    Ok(Outcome::new(result, lines))
}

/// `bead takeover`: replaces the current owner of each issue, after investigating; it must be
/// coordinated online and give its reason.
pub(crate) fn takeover(ctx: &Context<'_>, args: &OwnershipArgs) -> Result<Outcome, Failure> {
    if args.offline {
        return Err(Failure::new(
            Code::Validation,
            "a takeover has to be coordinated online",
        ));
    }
    let Some(reason) = args.reason.as_deref().filter(|r| !r.is_empty()) else {
        return Err(Failure::new(Code::Validation, "a takeover needs --reason"));
    };
    let step = ClaimStep {
        actor: ctx.actor,
        keys: &args.ids,
        takeover: Some(reason),
    };
    let signature = command_signature(ctx.actor, &args.asked, None);
    coordinated(
        ctx.ws,
        ctx.actor,
        args.offline,
        args.remote.as_deref(),
        &signature,
        |tx, mode| step.run(tx, mode),
    )?;
    let st = view(ctx.ws)?;
    still_held(ctx, &st, &args.ids)?;
    let result = details(&st, &args.ids)?;
    let reason = beadroll_core::text::printed_text(std::os::unix::ffi::OsStrExt::as_bytes(reason));
    let lines = result
        .iter()
        .map(|item| {
            let generation = item
                .get("claim")
                .and_then(|claim| claim.get("generation"))
                .map(str_of)
                .unwrap_or_default();
            let id = item.get("id").map(str_of).unwrap_or_default();
            format!("Took over {id} (generation {generation}): {reason}")
        })
        .collect();
    let result = Value::Array(result.into_iter().map(Value::from).collect());
    Ok(Outcome::new(result, lines))
}

/// `bead release`: gives up the actor's claim on each issue, or its offline claim.
pub(crate) fn release(ctx: &Context<'_>, args: &OwnershipArgs) -> Result<Outcome, Failure> {
    let signature = command_signature(ctx.actor, &args.asked, None);
    coordinated(
        ctx.ws,
        ctx.actor,
        args.offline,
        args.remote.as_deref(),
        &signature,
        |tx, mode| -> Result<Mode, Failure> {
            let st: State = tx.state()?.clone();
            need_tracker(&st)?;
            let me = ctx.actor.label(&tx.replica);
            for key in &args.ids {
                let issue = resolve_arg(&st, key)?;
                let uid = issue.uid().to_string();
                let held = issue.claim();
                if let Some(held) = held.filter(|held| me.is(&held.owner)) {
                    let mut data = Object::new();
                    data.insert("claim".into(), Value::from(held.id.to_string()));
                    let mut valid = true;
                    if let Some(reason) = args.reason.as_deref().filter(|r| !r.is_empty()) {
                        let (reason, ok) = arg_text(reason);
                        valid = ok;
                        data.insert("reason".into(), Value::from(reason));
                    }
                    tx.emit("release", Some(&uid), data, None)?;
                    encodable(valid)?;
                } else if issue.provisional_claims().iter().any(|p| me.is(&p.owner)) {
                    tx.emit("release", Some(&uid), Object::new(), None)?;
                } else {
                    // the label as it was given: Python's message holds its lone surrogates
                    let mut message = format!("{} is not claimed by ", issue.key()).into_bytes();
                    message.extend_from_slice(me.as_bytes());
                    if let Some(held) = held {
                        message.extend_from_slice(
                            format!("; it is held by {}", held.owner).as_bytes(),
                        );
                    }
                    return Err(Failure::bytes(Code::NotOwner, message));
                }
            }
            Ok(mode)
        },
    )?;
    let st = view(ctx.ws)?;
    let result: Vec<Object> = args
        .ids
        .iter()
        .map(|key| Ok(st.brief(resolve_arg(&st, key)?)))
        .collect::<Result<_, Failure>>()?;
    let lines = result
        .iter()
        .map(|item| {
            let field = |name: &str| item.get(name).map(str_of).unwrap_or_default();
            format!("Released {}: {}", field("id"), field("title"))
        })
        .collect();
    let result = Value::Array(result.into_iter().map(Value::from).collect());
    Ok(Outcome::new(result, lines))
}
