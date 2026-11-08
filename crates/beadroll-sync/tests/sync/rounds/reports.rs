//! A rejected publication contributes incoming progress but not its round's confirmations.

use std::path::{Path, PathBuf};

use beadroll_store::{Endpoint, claim_files, read_attempt};
use beadroll_sync::synchronise;
use beadroll_test_support::Machine;

use crate::support::{Gate, alongside, sandbox, text, workspace};

/// A pending twin of an already published attempt models a lost confirmation before recovery.
fn pending_twin(store: &Path, accepted: &Path) -> PathBuf {
    let pending = store.join("pending");
    std::fs::create_dir_all(&pending).unwrap();
    let twin = pending.join(accepted.file_name().unwrap());
    std::fs::copy(accepted, &twin).unwrap();
    twin
}

#[test]
fn a_retry_reports_only_its_final_confirmations_and_all_incoming_events() {
    let sandbox = sandbox();
    let a = Machine::new(&sandbox, "a");
    let b = Machine::new(&sandbox, "b");
    a.bead(["init", "--prefix", "ex"]);
    let key = text(&a.bead(["create", "owned here"]).unwrap()["id"]);
    a.bead(["claim", &key]);
    a.bead(["release", &key]);
    b.bead(["sync"]);
    // Each round receives a new sequential event; a's edit requires an actual push.
    a.bead(["create", "local edit", "--offline"]);
    b.bead(["create", "before the first merge"]);

    let publishing = Gate::new(&sandbox, "publishing");
    let backoff = Gate::new(&sandbox, "backoff");
    let faults = format!(
        "publish.before={};backoff={}",
        publishing.action(),
        backoff.action()
    );
    let ws = workspace(&a, sandbox.origin(), &[("BEAD_FAILPOINTS", &faults)]);
    let attempts = claim_files(&ws).unwrap();
    let attempt = |op: &str| {
        attempts
            .iter()
            .find(|path| read_attempt(path, None)[0]["op"].as_str() == Some(op))
            .unwrap()
    };
    let first = attempt("claim");
    let last = attempt("release");
    let first_id = text(&read_attempt(first, None)[0]["id"]);
    let last_id = text(&read_attempt(last, None)[0]["id"]);
    assert_ne!(first_id, last_id);
    let first_pending = pending_twin(&ws.dir, first);

    let (result, met) = alongside(
        || synchronise(&ws, &Endpoint::at(sandbox.origin())),
        |done| {
            let published = publishing.meet(done, || {
                assert!(
                    !first_pending.exists(),
                    "the first merge confirmed its twin"
                );
                // Advance the remote while a holds its old head, so its push is rejected.
                b.bead(["create", "after the first merge"]);
            });
            let retried = backoff.meet(done, || {
                // The store is unlocked here; the next merge will confirm a different twin.
                pending_twin(&ws.dir, last)
            });
            (published, retried)
        },
    );
    assert!(
        met.0.is_some() && met.1.is_some(),
        "both round boundaries were reached"
    );
    let synced = result.unwrap();
    assert_eq!((synced.attempts, synced.pulled, synced.pushed), (2, 2, 1));
    assert_eq!(synced.confirmed, [last_id]);
    assert!(
        !met.1.unwrap().exists(),
        "the final merge confirmed its twin"
    );
    assert_eq!(ws.settings.failpoints.hits("publish.before"), 2);
    assert_eq!(ws.settings.failpoints.hits("backoff"), 1);
    b.bead(["sync"]);
    assert_eq!(a.export(), b.export());
}
