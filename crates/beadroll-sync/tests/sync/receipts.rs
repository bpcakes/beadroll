//! An accepted receipt carries commitment across invocations, even when the confirming command
//! failed before it could retire its pending twin.

use beadroll_core::code::Code;
use beadroll_store::{glob_ending, pending_files};
use beadroll_sync::Mode;
use beadroll_test_support::Machine;

use crate::support::{actor, claim_command, sandbox, text, workspace};

#[test]
fn a_confirmed_attempts_receipt_recovers_commitment_without_another_transaction() {
    let sandbox = sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let key = text(&a.bead(["create", "x"]).expect("an issue")["id"]);
    let actor = actor(&a);
    let ws = workspace(
        &a,
        sandbox.origin(),
        &[("BEAD_FAILPOINTS", "ownership.after-confirm=error(IO)")],
    );
    let error = claim_command(&ws, &actor, &key).expect_err("stopped after confirmation");
    assert_eq!(error.code(), Code::Io);
    assert_eq!(ws.settings.failpoints.hits("ownership.after-confirm"), 1);
    assert!(ws.committed(), "the error does not undo confirmation");
    assert!(
        !ws.wrote(),
        "confirmation failed before publication bookkeeping"
    );
    assert_eq!(pending_files(&ws).len(), 1);
    assert_eq!(glob_ending(&ws.dir.join("pending"), ".command").len(), 1);
    let accepted = a.show(&key)["claim"].clone();
    assert_eq!(accepted["agent"], actor.value["agent"]);
    assert_eq!(accepted["thread"], actor.value["thread"]);

    let retry = workspace(
        &a,
        sandbox.origin(),
        &[("BEAD_FAILPOINTS", "ownership.before-transaction=exit(97)")],
    );
    assert!(
        !retry.committed(),
        "a new invocation has no recorded effect yet"
    );
    assert_eq!(
        claim_command(&retry, &actor, &key).expect("recognised retry"),
        Mode::Online
    );
    assert!(
        retry.committed(),
        "the accepted receipt belongs to this request"
    );
    assert!(!retry.wrote());
    assert_eq!(
        retry
            .settings
            .failpoints
            .hits("ownership.before-transaction"),
        0
    );
    assert!(pending_files(&retry).is_empty());
    assert!(glob_ending(&retry.dir.join("pending"), ".command").is_empty());
    assert_eq!(a.show(&key)["claim"], accepted);
}
