//! `Protocol3`: an attempt is one file, confirmed by one write; a fence beats a push still in
//! flight; an acknowledgement lost after the remote took an update is recovered, never applied
//! twice (the third independent review, 2026-10-06).

use std::os::unix::fs::PermissionsExt as _;
use std::time::{Duration, Instant};

use beadroll_store::{Workspace, write_attempt_file};
use beadroll_sync::{Error, Link, Mode, PushOutcome, Tx, locked, synchronise};
use beadroll_test_support::{Json, Machine, Sandbox, git};

use crate::support::{
    Gate, Open, actor, alongside, claim, claiming_update, comments, pending_ids, release_command,
    sandbox, text, workspace,
};

/// Machine a's tracker with one issue, "before", synced to machine b.
struct Protocol3 {
    sandbox: Sandbox,
    a: Machine,
    b: Machine,
    key: String,
}

fn setup() -> Protocol3 {
    let sandbox = sandbox();
    let a = Machine::new(&sandbox, "protocol3-a");
    a.bead(["init", "--prefix", "ex"]);
    let key = text(&a.bead(["create", "before"]).unwrap()["id"]);
    a.bead(["sync"]);
    let b = Machine::new(&sandbox, "protocol3-b");
    b.bead(["sync"]);
    Protocol3 { sandbox, a, b, key }
}

impl Protocol3 {
    /// Machine a's store in this process, with these failpoints.
    fn ws(&self, failpoints: &str) -> Workspace {
        workspace(
            &self.a,
            self.sandbox.origin(),
            &[("BEAD_FAILPOINTS", failpoints)],
        )
    }

    /// The commit the remote's coordination ref holds.
    fn remote_tip(&self) -> String {
        let origin = self.sandbox.origin().as_os_str();
        let args = [
            "--git-dir".as_ref(),
            origin,
            "rev-parse".as_ref(),
            "refs/beads/journal".as_ref(),
        ];
        git(args, None).trim().to_owned()
    }
}

/// `path` quoted for the shell.
fn quoted(path: &std::path::Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
}

/// Whether the issue shows a claim, its title and its labels.
fn seen(issue: &Json) -> (bool, String, Vec<String>) {
    let labels = issue["labels"].as_array().unwrap_or_default();
    (
        issue.get("claim").is_some(),
        text(&issue["title"]),
        labels.iter().map(text).collect(),
    )
}

/// The first actual receive-pack stalls; the fence commit that follows is accepted first.
#[test]
fn fence_wins_against_a_real_push_still_running_after_timeout() {
    let s = setup();
    let root = s.sandbox.root();
    let (gate, first, finished) = (
        root.join("release-first-hook"),
        root.join("first-hook"),
        root.join("hook-finished"),
    );
    let hook = s.sandbox.origin().join("hooks").join("pre-receive");
    let script = format!(
        "#!/bin/sh\nif mkdir {} 2>/dev/null; then\n n=0\n while [ ! -f {} ] && [ \"$n\" -lt 200 ]; \
         do sleep .05; n=$((n+1)); done\n touch {}\nfi\nexit 0\n",
        quoted(&first),
        quoted(&gate),
        quoted(&finished)
    );
    std::fs::write(&hook, script).unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    let _release = Open(gate.clone());
    let origin = s.sandbox.origin().as_os_str();
    let ws = workspace(&s.a, s.sandbox.origin(), &[]);
    // the per-Link timeout: this one publish waits a quarter of a second
    let short = workspace(&s.a, s.sandbox.origin(), &[("BEAD_NET_TIMEOUT", "0.25")]);
    let actor = actor(&s.a);
    let outcome = locked(&ws, || -> Result<PushOutcome, Error> {
        let mut link = Link::new(&ws, origin)?;
        let tip = link.fetch()?;
        link.merge(tip.as_deref())?;
        let mut tx = Tx::new(&ws, &actor)?;
        claim(&mut tx, Mode::Online, &s.key)?;
        let id = text(&tx.ownership[0]["id"]);
        let pending = ws.dir.join("pending").join(format!("{id}.json"));
        write_attempt_file(&pending, &tx.ownership, &ws.settings.failpoints)?;
        Link::new(&short, origin)?.publish(tip.as_deref(), "claim", &tx.ownership, false)
    })
    .unwrap();
    assert_eq!(outcome, PushOutcome::Uncertain);
    assert!(first.exists());
    assert!(!finished.exists());
    assert_eq!(pending_ids(&ws).len(), 1);
    assert!(s.a.show(&s.key).get("claim").is_none());
    s.a.bead(["comments", "add", &s.key, "independent local work"]);
    let synced = synchronise(&ws, origin).unwrap();
    let fenced = synced.tip.unwrap();
    assert_eq!(pending_ids(&ws), Vec::<String>::new());
    std::fs::write(&gate, b"").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !finished.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(finished.exists());
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(s.remote_tip(), fenced);
    s.b.bead(["sync"]);
    s.a.bead(["sync"]);
    assert!(s.a.show(&s.key).get("claim").is_none());
    assert!(s.b.show(&s.key).get("claim").is_none());
    assert_eq!(s.a.export(), s.b.export());
    assert_eq!(comments(&s.a.show(&s.key)), ["independent local work"]);
    assert_eq!(
        s.a.bead(["info"]).unwrap()["integrity"],
        Json::Array(Vec::new())
    );
}

/// A real reader runs around the local confirmation write of an accepted attempt: an attempt is
/// one file, so the reader sees nothing of it or all of it.
#[test]
fn reader_sees_all_or_none_of_accepted_claim_and_bundled_edits() {
    let s = setup();
    let (writing, written) = (
        Gate::new(&s.sandbox, "confirming"),
        Gate::new(&s.sandbox, "confirmed"),
    );
    let ws = s.ws(&format!(
        "atomic-write:claims/@always={};atomic-written:claims/@always={}",
        writing.action(),
        written.action()
    ));
    let actor = actor(&s.a);
    let (result, observed) = alongside(
        || claiming_update(&ws, &actor, &s.key, "after", Some("accepted")),
        |done| {
            let before = writing.meet(done, || s.a.show(&s.key));
            let after = written.meet(done, || s.a.show(&s.key));
            [before, after]
        },
    );
    result.unwrap();
    assert_eq!(
        ws.settings.failpoints.hits("atomic-write:claims/"),
        1,
        "the attempt must be confirmed by exactly one write"
    );
    let observed: Vec<Json> = observed.into_iter().flatten().collect();
    assert_eq!(observed.len(), 2, "the reader did not run around the write");
    let none = (false, "before".to_owned(), Vec::new());
    let all = (true, "after".to_owned(), vec!["accepted".to_owned()]);
    for current in &observed {
        let current = seen(current);
        assert!(
            current == none || current == all,
            "accepted atomic update is visible as a confirmed claim with its bundled edits \
             missing: {current:?}"
        );
    }
    let last = seen(&observed[1]);
    assert_eq!((last.1, last.2), (all.1, all.2));
}

#[test]
fn recovered_atomic_update_is_not_applied_again_after_a_later_observed_edit() {
    let s = setup();
    // the remote takes the update, the answer is lost; between the rounds this machine syncs,
    // sees the update, and edits the issue again
    let gate = Gate::new(&s.sandbox, "backoff");
    let ws = s.ws(&format!("push.reply=fail-reply;backoff={}", gate.action()));
    let actor = actor(&s.a);
    let (result, written) = alongside(
        || claiming_update(&ws, &actor, &s.key, "accepted first", None),
        |done| {
            gate.meet(done, || {
                s.a.bead(["sync"]);
                let title = s.a.show(&s.key)["title"].clone();
                s.a.bead(["update", &s.key, "--title", "later observed edit"]);
                title
            })
        },
    );
    result.unwrap();
    assert_eq!(ws.settings.failpoints.hits("push.reply"), 1);
    assert_eq!(
        written
            .and_then(|t| t.as_str().map(str::to_owned))
            .as_deref(),
        Some("accepted first")
    );
    s.b.bead(["sync"]);
    s.a.bead(["sync"]);
    assert_eq!(
        s.a.show(&s.key)["title"].as_str(),
        Some("later observed edit"),
        "acknowledgement recovery executes the already accepted bundled edit again, overwriting \
         later work that had seen it"
    );
}

#[test]
fn release_recovers_a_successful_push_with_lost_acknowledgement() {
    let s = setup();
    s.a.bead(["claim", &s.key]);
    let ws = s.ws("push.reply=fail-reply;backoff@always=skip");
    let result = release_command(&ws, &actor(&s.a), &s.key);
    // one push, its answer lost, and one more round, which finds it accepted
    assert_eq!(ws.settings.failpoints.hits("push.reply"), 1);
    assert_eq!(ws.settings.failpoints.hits("backoff"), 1);
    s.b.bead(["sync"]);
    assert!(s.b.show(&s.key).get("claim").is_none());
    assert!(s.a.show(&s.key).get("claim").is_none());
    assert_eq!(pending_ids(&ws), Vec::<String>::new());
    assert!(
        result.is_ok(),
        "accepted release recovered from remote but command reports {result:?}"
    );
}
