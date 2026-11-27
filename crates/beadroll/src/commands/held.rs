//! What a command hands the command line besides its answer (`Outcome::held`): a write that goes
//! on to neither read a state nor publish hands over the transaction it was made in, to be kept
//! until the process exits (roll-lqjj.42); one that publishes what it wrote itself, or fails,
//! hands over nothing, having let go of it first, unless it had nowhere to publish to, which leaves
//! nothing to read and lets it keep what it held (roll-e3pu).

use std::ffi::OsString;

use beadroll_store::Workspace;
use beadroll_sync::NetworkMode;
use beadroll_test_support::Machine;

use super::{
    CommentArgs, Context, CreateArgs, Failure, Outcome, OwnershipArgs, Transition, TransitionArgs,
    UpdateArgs, claim, comments, create, dep, import, transition, update,
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

/// `close`, `reopen` or `delete` of `key`, published at once unless `network` is offline.
fn transit(
    ctx: &Context<'_>,
    op: Transition,
    key: &OsString,
    network: NetworkMode,
) -> Result<Outcome, Failure> {
    let args = TransitionArgs {
        ids: vec![key.clone()],
        reason: None,
        recursive: false,
        force: false,
        network,
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
        assert!(
            holds(dep(
                &ctx,
                &edge,
                None,
                beadroll_core::query::Edges::WaitsFor
            )),
            "dep {action}"
        );
    }
    for op in [Transition::Close, Transition::Reopen, Transition::Delete] {
        assert!(
            holds(transit(&ctx, op, &two, NetworkMode::Offline)),
            "{op:?}"
        );
    }
    let file = sandbox.root().join("issues.jsonl");
    std::fs::write(
        &file,
        "{\"id\":\"ex-90\",\"title\":\"imported\",\"status\":\"open\"}\n",
    )
    .expect("an export");
    assert!(
        holds(import(
            &ctx,
            file.as_os_str(),
            crate::commands::ImportMode::Apply
        )),
        "import"
    );
    let retitle = UpdateArgs {
        ids: vec![one],
        title: Some("retitled".into()),
        fields: Default::default(),
        claim: false,
        force: false,
        add_label: Vec::new(),
        remove_label: Vec::new(),
        set_labels: None,
        network: NetworkMode::Offline,
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
        network: NetworkMode::Online,
        remote: None,
        asked: "{}".to_owned(),
    };
    claim(&ctx, &taking).expect("a claim");
    let ended = transit(&ctx, Transition::Close, &key, NetworkMode::Online)
        .expect("a close that ends the claim");
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

/// The times the store's state was read through `ws` (`projection.read`) while `run` ran, and
/// what `run` came to.
fn reads<T>(ws: &Workspace, run: impl FnOnce() -> T) -> (u64, T) {
    let before = ws.settings.failpoints.hits("projection.read");
    let done = run();
    (
        ws.settings.failpoints.hits("projection.read") - before,
        done,
    )
}

/// `update KEY --status closed`, published at once unless `network` is offline.
fn closing_update(key: &OsString, network: NetworkMode) -> UpdateArgs {
    let fields = super::ContentArgs {
        status: Some("closed".into()),
        ..Default::default()
    };
    UpdateArgs {
        ids: vec![key.clone()],
        title: None,
        fields,
        claim: false,
        force: false,
        add_label: Vec::new(),
        remove_label: Vec::new(),
        set_labels: None,
        network,
        remote: None,
        asked: "{}".to_owned(),
    }
}

/// A write that publishes itself before it answers has nothing to publish to `--offline`, and the
/// store then holds what the write left it: `create --parent`, and a reopen and an `update --status
/// closed` that end a claim, read the store's state no more often than the same write that
/// publishes nothing, and hand over what they held (roll-e3pu). They let go of it and read the
/// store again, as though a merge could have renumbered the child or taken the claim over. A
/// close or delete that names the claim it ends still asks the store whether replay took it
/// (`superseded_owner_close_says_so`).
#[test]
fn a_write_with_nowhere_to_publish_itself_reads_no_state_after_it() {
    let (sandbox, machine) = tracker();
    let parent = issue(&machine, "parent");
    machine.bead(["claim", parent.to_str().expect("a key"), "--offline"]);
    let [claimed, updated, plain, other] =
        ["claimed", "updated", "plain", "other"].map(|title| issue(&machine, title));
    for key in [&claimed, &updated] {
        machine.bead(["claim", key.to_str().expect("a key")]);
    }
    let counted = [("BEAD_FAILPOINTS", "projection.read@nth(1000000)=sleep(0)")];
    let ws = workspace(&machine, sandbox.origin(), &counted);
    let actor = actor(&machine);
    let ctx = Context {
        ws: &ws,
        actor: &actor,
    };
    let offline = |title: &str, parent: Option<&OsString>| {
        let fields = super::ContentArgs {
            parent: parent.cloned(),
            ..Default::default()
        };
        CreateArgs {
            title_pos: Some(title.into()),
            fields,
            network: NetworkMode::Offline,
            ..CreateArgs::default()
        }
    };
    let (alone, made) = reads(&ws, || create(&ctx, &offline("alone", None)));
    assert!(holds(made), "create");
    let (child, made) = reads(&ws, || create(&ctx, &offline("child", Some(&parent))));
    let made = made.expect("create --parent");
    assert!(made.held.is_some(), "create --parent handed nothing over");
    // the answer is the detail the store's view gives once the write is in, as it was when the
    // child read the store again: with what the view lists of this machine's attempts, the
    // parent's offline claim among them
    let shown = {
        let st = beadroll_store::view_outline(&ws).expect("the store's state");
        let named = |issue: &&beadroll_core::state::Issue| issue.title() == "child";
        let uid = st.issues().find(named).expect("the child").uid();
        let st = super::texts_in(&ws, st, [uid]).expect("its texts");
        let issue = st.issue(uid).expect("the child");
        beadroll_core::json::Value::from(st.detail(issue.hydrated().expect("its texts")))
    };
    assert_eq!(
        made.result.as_ref(),
        Some(&shown),
        "the answer of create --parent"
    );
    // the one read is the transaction's own, of the projection it writes on
    assert_eq!(alone, 1, "create read the store's state again");
    assert_eq!(child, alone, "create --parent read the store again");
    machine.bead(["close", plain.to_str().expect("a key")]);
    let (unclaimed, reopened) = reads(&ws, || {
        transit(&ctx, Transition::Reopen, &plain, NetworkMode::Offline)
    });
    assert!(holds(reopened), "reopen");
    let (ending, reopened) = reads(&ws, || {
        transit(&ctx, Transition::Reopen, &claimed, NetworkMode::Offline)
    });
    assert!(
        holds(reopened),
        "a reopen that ends a claim handed nothing over"
    );
    assert_eq!(unclaimed, 1, "reopen read the store's state again");
    assert_eq!(
        ending, unclaimed,
        "a reopen that ends a claim read the store again"
    );
    let (unclaimed, closed) = reads(&ws, || {
        update(&ctx, &closing_update(&other, NetworkMode::Offline))
    });
    closed.expect("update --status closed");
    let (ending, closed) = reads(&ws, || {
        update(&ctx, &closing_update(&updated, NetworkMode::Offline))
    });
    assert!(
        holds(closed),
        "an update that ends a claim handed nothing over"
    );
    assert_eq!(unclaimed, 1, "update read the store's state again");
    assert_eq!(
        ending, unclaimed,
        "an update that ends a claim read the store again"
    );
}
