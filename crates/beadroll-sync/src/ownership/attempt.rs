//! A prepared attempt owns the events and the answer a lost acknowledgement must recover. Its
//! files outlive this value: an error or process exit leaves recovery to a later fetch and fence.

use beadroll_core::code::Code;
use beadroll_core::json::Value;
use beadroll_git::PushOutcome;
use beadroll_store::{
    FileMode, Fired, Workspace, attempt_path, pending_dir, pending_paths, private_dirs,
    write_atomic, write_attempt_file,
};

use super::Mode;
use crate::error::Error;
use crate::link::{Link, PublishMode};
use crate::tx::{confirm_ownership, drop_pending};

pub(super) enum Recovery {
    Ready,
    Awaiting(PendingAttempt),
}

impl Recovery {
    /// After merge, a confirmed attempt answers without rebuilding the command. An absent file
    /// does not mean rejection: only a successful fence permits the caller to become ready again.
    pub(super) fn confirmed(&self, ws: &Workspace) -> Result<Option<Mode>, Error> {
        match self {
            Self::Awaiting(pending) if attempt_path(ws, &pending.events).exists() => {
                drop_pending(ws, std::slice::from_ref(&pending.events))?;
                Ok(Some(pending.answer))
            }
            Self::Ready | Self::Awaiting(_) => Ok(None),
        }
    }
}

pub(super) enum AttemptOutcome {
    Confirmed(Mode),
    Rejected,
    Refused,
    Uncertain(PendingAttempt),
}

pub(super) struct PendingAttempt {
    events: Vec<Value>,
    answer: Mode,
}

impl PendingAttempt {
    /// The receipt is durable before the pending attempt, as required by retry recognition.
    pub(super) fn prepare(
        ws: &Workspace,
        events: Vec<Value>,
        signature: &str,
        answer: Mode,
    ) -> Result<Self, Error> {
        let (attempt, receipt) = pending_paths(ws, &events).ok_or_else(|| {
            Error::Validation("an attempt whose first event has no id".to_owned())
        })?;
        private_dirs(&pending_dir(ws))?;
        write_atomic(
            &receipt,
            signature.as_bytes(),
            FileMode::Private,
            &ws.settings.failpoints,
        )?;
        write_attempt_file(ws, &attempt, &events)?;
        Ok(Self { events, answer })
    }

    /// Acceptance confirms locally before retiring the pending twin. A definite refusal or
    /// rejection retires it immediately; uncertainty retains both files and the saved answer.
    pub(super) fn publish(
        self,
        ws: &Workspace,
        link: &mut Link<'_>,
        tip: Option<&str>,
    ) -> Result<AttemptOutcome, Error> {
        let failpoints = &ws.settings.failpoints;
        failpoints.hit("ownership.before-publish");
        failpoints.hit("claim.before-push");
        let outcome = match link.publish(tip, "claim", &self.events, PublishMode::IfChanged) {
            Ok(outcome) => outcome,
            Err(error) if error.code() == Code::RemoteUnreachable => PushOutcome::Uncertain,
            Err(error) => return Err(error),
        };
        match outcome {
            PushOutcome::Ok => {
                failpoints.hit("ownership.after-publish-ok");
                // the remote holds it now: a failure here is no `IO` that left nothing half-done
                if let Err(error) = confirm_ownership(ws, &self.events) {
                    return Err(Error::Store(beadroll_store::Error::io_message(format!(
                        "the remote accepted this, so it stands, but recording it here failed \
                         ({error}): run `bead sync` to record it here"
                    ))));
                }
                if let Some(Fired::Error(code)) = failpoints.hit("ownership.after-confirm") {
                    return Err(Error::injected(code, "ownership.after-confirm"));
                }
                drop_pending(ws, std::slice::from_ref(&self.events))?;
                Ok(AttemptOutcome::Confirmed(self.answer))
            }
            PushOutcome::Refused | PushOutcome::Rejected => {
                drop_pending(ws, std::slice::from_ref(&self.events))?;
                Ok(if outcome == PushOutcome::Refused {
                    AttemptOutcome::Refused
                } else {
                    AttemptOutcome::Rejected
                })
            }
            PushOutcome::Uncertain => Ok(AttemptOutcome::Uncertain(self)),
        }
    }
}
