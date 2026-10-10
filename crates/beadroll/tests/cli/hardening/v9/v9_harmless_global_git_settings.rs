//! Global git settings reach sync.git without harm: thirteen of them, each in a fresh two-machine
//! round.

use beadroll_test_support::{Machine, code};

use super::scratch_git_config;

/// commit-tree does not honour commit.gpgSign, so a bad signing key is never used.
const SIGNING_AND_PUSH: [&str; 5] = [
    "[commit]\n\tgpgSign = true\n[gpg]\n\tformat = ssh\n[user]\n\tsigningKey = /nonexistent\n",
    "[push]\n\tgpgSign = if-asked\n",
    "[push]\n\tdefault = nothing\n",
    "[transfer]\n\tfsckObjects = true\n[receive]\n\tfsckObjects = true\n",
    "[fetch]\n\tprune = true\n\tpruneTags = true\n",
];

const PROTOCOL_AND_REPOSITORY: [&str; 4] = [
    "[protocol]\n\tversion = 0\n",
    "[protocol]\n\tversion = 1\n",
    "[safe]\n\tbareRepository = explicit\n",
    "[core]\n\tfsmonitor = true\n",
];

const FORMAT_AND_HOUSEKEEPING: [&str; 4] = [
    "[init]\n\tdefaultRefFormat = reftable\n",
    "[gc]\n\tauto = 1\n\tautoDetach = false\n",
    "[core]\n\tautocrlf = true\n\tsafecrlf = true\n",
    "[color]\n\tui = always\n",
];

/// `_v9_round`: a two-machine round under one scratch global git config holding `text`: init,
/// create, sync, claim on b, a competing claim on a (ALREADY_CLAIMED), close on b, sync both; then
/// both machines agree.
fn round(text: &str) {
    let sandbox = crate::sandbox();
    let genv = scratch_git_config(&sandbox, text);
    let mut a = Machine::new(&sandbox, "a");
    let mut b = Machine::new(&sandbox, "b");
    for machine in [&mut a, &mut b] {
        machine.extra.extend(genv.clone());
    }
    a.bead(["init"]);
    let created = a.bead(["create", "one"]).expect("the created issue");
    let issue = created["id"].as_str().expect("an id").to_owned();
    a.bead(["sync"]);
    b.bead(["sync"]);
    b.bead(["claim", &issue]);
    let competing = a.run(["claim", &issue, "--json"]);
    assert_eq!(code(&competing), "ALREADY_CLAIMED", "{text}");
    b.bead(["close", &issue, "-r", "done"]);
    a.bead(["sync"]);
    b.bead(["sync"]);
    assert_eq!(a.export(), b.export(), "{text}");
    assert_eq!(a.bead(["show", &issue]), b.bead(["show", &issue]), "{text}");
    let shown = a.bead(["show", &issue]).expect("show prints a list");
    assert_eq!(shown[0]["status"].as_str(), Some("closed"), "{text}");
}

/// Verified (V9): global git settings reach sync.git without harm. In a two-machine round (init,
/// create, sync, claim, a competing claim gets ALREADY_CLAIMED, close, sync both) the exports and
/// shows are identical and the status is closed. This part covers commit.gpgSign with
/// gpg.format=ssh and a nonexistent key (harmless because `git commit-tree` does not honour
/// commit.gpgSign: a direct check printed a commit id with rc=0), push.gpgSign=if-asked,
/// push.default=nothing, transfer/receive.fsckObjects and fetch.prune+pruneTags. The review
/// measured 13/13 settings harmless in this round, and 22/24 in its wider experiment (the failures
/// were push.gpgSign=true against a remote without signed-push support, as the user's own push
/// fails, and the no-setting control).
#[test]
fn signing_push_and_fetch_settings_are_harmless() {
    for text in SIGNING_AND_PUSH {
        round(text);
    }
}

/// Verified (V9): global git settings reach sync.git without harm, as above (two-machine round
/// with ALREADY_CLAIMED on the competing claim, identical exports and shows, status closed). This
/// part covers protocol.version=0, protocol.version=1, safe.bareRepository=explicit and
/// core.fsmonitor=true. The review measured 13/13 settings harmless in this round and 22/24 in
/// its wider experiment.
#[test]
fn protocol_and_repository_settings_are_harmless() {
    for text in PROTOCOL_AND_REPOSITORY {
        round(text);
    }
}

/// Verified (V9): global git settings reach sync.git without harm, as above (two-machine round
/// with ALREADY_CLAIMED on the competing claim, identical exports and shows, status closed). This
/// part covers init.defaultRefFormat=reftable (the planned Git 3.0 default), gc.auto=1 with
/// gc.autoDetach=false, core.autocrlf+safecrlf and color.ui=always. The review measured 13/13
/// settings harmless in this round and 22/24 in its wider experiment.
#[test]
fn format_and_housekeeping_settings_are_harmless() {
    for text in FORMAT_AND_HOUSEKEEPING {
        round(text);
    }
}
