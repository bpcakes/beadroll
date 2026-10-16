//! What a command hands the command line besides its answer (`Outcome::held`): a write that goes
//! on to neither read a state nor publish hands over the transaction it was made in, to be kept
//! until the process exits (roll-lqjj.42); one that publishes what it wrote itself, or fails,
//! hands over nothing, having let go of it first.

use std::ffi::OsString;

use beadroll_test_support::Machine;

use super::{
    CommentArgs, Context, CreateArgs, Failure, Outcome, OwnershipArgs, TransitionArgs, UpdateArgs,
    claim, comments, create, dep, import, transition, update,
};
use crate::testing::{actor, tracker, workspace};

/// An issue `machine` created, by its key.
fn issue(machine: &Machine, title: &str) -> OsString {
    let made = machine.bead(["create", title]).expect("an issue");
    OsString::from(made["id"].as_str().expect("a key"))
}

/// `words` as arguments.
fn words(words: &[&str]) -> Vec<OsString> {
    words.iter().map(OsString::from).collect()
}

/// `close`, `reopen` or `delete` of `key`, published at once unless `offline`.
fn transit(ctx: &Context<'_>, op: &str, key: &OsString, offline: bool) -> Result<Outcome, Failure> {
    let args = TransitionArgs {
        ids: vec![key.clone()],
        reason: None,
        recursive: false,
        force: false,
        offline,
        remote: None,
    };
    transition(ctx, op, &args)
}

/// Whether the command handed anything over, and it answered.
fn holds(outcome: Result<Outcome, Failure>) -> bool {
    outcome.expect("the command answers").held.is_some()
}

/// Every command that writes and answers from what it wrote, without a state or a publish of its
/// own in between, hands its transaction over, or the state it answers from: `create`, `comments
/// add`, `dep add` and `remove`, `close`, `reopen` and `delete`, `import` and `update`.
#[test]
fn a_write_hands_the_command_line_its_transaction_to_keep() {
    let (sandbox, machine) = tracker();
    let (one, two) = (issue(&machine, "one"), issue(&machine, "two"));
    let ws = workspace(&machine, sandbox.origin(), &[]);
    let actor = actor(&machine);
    let ctx = Context {
        ws: &ws,
        actor: &actor,
    };
    let made = create(
        &ctx,
        &CreateArgs {
            title_pos: Some("three".into()),
            ..CreateArgs::default()
        },
    );
    assert!(holds(made), "create");
    let said = CommentArgs {
        words: [words(&["add"]), vec![one.clone()], words(&["said"])].concat(),
        message: None,
        file: None,
        author: None,
    };
    assert!(holds(comments(&ctx, &said)), "comments add");
    for action in ["add", "remove"] {
        let edge = [words(&[action]), vec![one.clone(), two.clone()]].concat();
        assert!(holds(dep(&ctx, &edge, None, false)), "dep {action}");
    }
    for op in ["close", "reopen", "delete"] {
        assert!(holds(transit(&ctx, op, &two, true)), "{op}");
    }
    let file = sandbox.root().join("issues.jsonl");
    std::fs::write(
        &file,
        "{\"id\":\"ex-90\",\"title\":\"imported\",\"status\":\"open\"}\n",
    )
    .expect("an export");
    assert!(holds(import(&ctx, file.as_os_str(), false)), "import");
    let retitle = UpdateArgs {
        ids: vec![one],
        title: Some("retitled".into()),
        fields: Default::default(),
        claim: false,
        force: false,
        add_label: Vec::new(),
        remove_label: Vec::new(),
        set_labels: None,
        offline: true,
        remote: None,
        asked: "{}".to_owned(),
    };
    assert!(holds(update(&ctx, &retitle)), "update");
}

/// A close that ends a claim publishes at once, inside the command, and reads the store after:
/// it lets go of its transaction first, so that it holds one only for the publish, and hands over
/// nothing. A write that fails hands over nothing either.
#[test]
fn a_write_that_publishes_or_fails_hands_over_nothing() {
    let (sandbox, machine) = tracker();
    let key = issue(&machine, "claimed");
    let ws = workspace(&machine, sandbox.origin(), &[]);
    let actor = actor(&machine);
    let ctx = Context {
        ws: &ws,
        actor: &actor,
    };
    let taking = OwnershipArgs {
        ids: vec![key.clone()],
        reason: None,
        offline: false,
        remote: None,
        asked: "{}".to_owned(),
    };
    claim(&ctx, &taking).expect("a claim");
    let ended = transit(&ctx, "close", &key, false).expect("a close that ends the claim");
    assert!(
        ended.held.is_none(),
        "it held its transaction through the publish"
    );
    assert!(!ws.wrote(), "the close was published");
    let unknown = CommentArgs {
        words: words(&["add", "ex-nothing", "said"]),
        message: None,
        file: None,
        author: None,
    };
    assert!(comments(&ctx, &unknown).is_err(), "no such issue");
    drop(sandbox);
}
