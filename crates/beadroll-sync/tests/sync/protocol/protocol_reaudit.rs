//! `ProtocolReaudit`: a claim whose publish never reached the remote, and the fences that settle it
//! (the second independent audit, 2026-10-06).

use beadroll_core::code::Code;
use beadroll_store::{Endpoint, Workspace};
use beadroll_sync::synchronise;
use beadroll_test_support::{Json, Machine, Sandbox};

use crate::support::{
    Gate, actor, alongside, claim_command, claiming_update, comments, pending_ids, sandbox, text,
    workspace,
};

/// Every publish fails as if the remote never answered.
const UNREACHABLE: &str = "publish.result@always=error(REMOTE_UNREACHABLE)";

/// Machine a's tracker with one issue, synced to machine b.
struct Reaudit {
    sandbox: Sandbox,
    a: Machine,
    b: Machine,
    key: String,
}

fn setup() -> Reaudit {
    let sandbox = sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let key = text(&a.bead(["create", "ownership target"]).unwrap()["id"]);
    a.bead(["sync"]);
    let b = Machine::new(&sandbox, "b");
    b.bead(["sync"]);
    Reaudit { sandbox, a, b, key }
}

impl Reaudit {
    /// Machine a's store in this process, with these failpoints.
    fn ws(&self, failpoints: &str) -> Workspace {
        workspace(
            &self.a,
            self.sandbox.origin(),
            &[("BEAD_FAILPOINTS", failpoints)],
        )
    }

    /// A claim, or with `title` a claiming update that also sets the title and adds `label`, whose
    /// publish never reaches the remote: it fails, and stays pending, unconfirmed.
    fn interrupted_claim(&self, update: Option<(&str, &str)>) -> Vec<String> {
        let ws = self.ws(UNREACHABLE);
        let actor = actor(&self.a);
        let result = match update {
            Some((title, label)) => claiming_update(&ws, &actor, &self.key, title, Some(label)),
            None => claim_command(&ws, &actor, &self.key),
        };
        assert!(result.is_err(), "{result:?}");
        assert!(self.a.show(&self.key).get("claim").is_none());
        pending_ids(&ws)
    }

    /// A sync of machine a in this process, its every publish failing as unreachable.
    fn failed_sync(&self) {
        let ws = self.ws(UNREACHABLE);
        let error = synchronise(&ws, &Endpoint::at(self.sandbox.origin())).unwrap_err();
        assert_eq!(error.code(), Code::RemoteUnreachable, "{error}");
    }

    /// Both machines sync twice; then they agree, and the issue is what they agree on.
    fn settle(&self) -> Json {
        for machine in [&self.a, &self.b, &self.a, &self.b] {
            machine.bead(["sync"]);
        }
        assert_eq!(self.a.export(), self.b.export());
        assert_eq!(self.a.show(&self.key), self.b.show(&self.key));
        self.a.show(&self.key)
    }
}

#[test]
fn refused_void_publish_must_not_activate_unaccepted_claim() {
    let s = setup();
    s.interrupted_claim(None);
    let note = "local work after failed claim";
    s.a.bead(["comments", "add", &s.key, note, "--offline"]);
    // b claims online while a's sync is about to publish on the head it fetched
    let gate = Gate::new(&s.sandbox, "publish");
    let ws = s.ws(&format!(
        "publish.before={};backoff@always=skip",
        gate.action()
    ));
    let (report, accepted) = alongside(
        || synchronise(&ws, &Endpoint::at(s.sandbox.origin())),
        |done| {
            gate.meet(done, || {
                s.b.bead(["claim", &s.key]).unwrap()[0]["claim"].clone()
            })
        },
    );
    let accepted = accepted.expect("a's sync published");
    assert!(report.unwrap().attempts >= 2);
    let last = s.settle();
    assert_eq!(
        last["claim"], accepted,
        "failed A claim displaced the claim B actually acquired online after void publication \
         lost a CAS race"
    );
}

#[test]
fn failed_void_publish_retry_must_not_activate_unaccepted_claim() {
    let s = setup();
    s.interrupted_claim(None);
    let note = "local work after failed claim";
    s.a.bead(["comments", "add", &s.key, note, "--offline"]);
    s.failed_sync();
    let accepted = s.b.bead(["claim", &s.key]).unwrap()[0]["claim"].clone();
    let last = s.settle();
    assert_eq!(
        last["claim"], accepted,
        "retry took back only the void and permanently activated original unaccepted claim"
    );
}

#[test]
fn failed_void_push_must_leave_original_claim_unconfirmed() {
    let s = setup();
    s.interrupted_claim(None);
    let note = "local work after failed claim";
    s.a.bead(["comments", "add", &s.key, note, "--offline"]);
    s.failed_sync();
    assert!(
        s.a.show(&s.key).get("claim").is_none(),
        "unconfirmed void masks itself but unmasks original claim while remote never accepted \
         either"
    );
}

#[test]
fn failed_multievent_update_void_retry_must_not_commit_edits() {
    let s = setup();
    s.interrupted_claim(Some(("unaccepted title", "unaccepted")));
    s.a.bead(["comments", "add", &s.key, "independent work"]);
    s.failed_sync();
    let last = s.settle();
    assert_eq!(
        last["title"].as_str(),
        Some("ownership target"),
        "failed atomic update edits revived when retry rolled back their void records"
    );
    let labels = last["labels"].as_array().unwrap_or_default();
    assert!(!labels.contains(&Json::from("unaccepted")));
    assert!(last.get("claim").is_none());
}

#[test]
fn quiet_reconcile_of_unaccepted_claim_preserves_independent_work() {
    let s = setup();
    s.interrupted_claim(None);
    s.a.bead(["comments", "add", &s.key, "independent work"]);
    let last = s.settle();
    assert!(last.get("claim").is_none());
    assert_eq!(comments(&last), ["independent work"]);
    assert_eq!(
        s.a.bead(["info"]).unwrap()["integrity"],
        Json::Array(Vec::new())
    );
}

#[test]
fn nonzero_applied_push_with_interleaved_writer_must_not_fork() {
    let s = setup();
    // the remote takes the claim, the answer is lost; a writer takes the lock between the rounds
    let gate = Gate::new(&s.sandbox, "backoff");
    let ws = s.ws(&format!("push.reply=fail-reply;backoff={}", gate.action()));
    let actor = actor(&s.a);
    let (result, written) = alongside(
        || claim_command(&ws, &actor, &s.key),
        |done| {
            gate.meet(done, || {
                let note = "writer acquires lock between retries";
                s.a.bead(["comments", "add", &s.key, note]);
            })
        },
    );
    assert_eq!(ws.settings.failpoints.hits("push.reply"), 1);
    assert!(
        written.is_some(),
        "no round followed the lost acknowledgement"
    );
    s.b.bead(["sync"]);
    assert!(s.b.show(&s.key).get("claim").is_some());
    assert!(
        result.is_ok(),
        "accepted claim was rolled back on ambiguous git failure; local writer now forks the \
         journal: {result:?}"
    );
}
