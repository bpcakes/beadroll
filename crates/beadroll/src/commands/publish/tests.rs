//! Where a write's own publish goes decides what becomes of what the write held: kept and handed
//! back when nothing is exchanged, let go of before an exchange.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use beadroll_core::json::{Object, Value};
use beadroll_sync::NetworkMode;
use beadroll_test_support::{Machine, Sandbox, built_bead, git};

use super::{Published, publish_first};
use crate::testing::{tracker, without_origin, workspace};

/// What a write held, which counts in `freed` the times it is let go of.
struct Probe(Arc<AtomicUsize>);

impl Drop for Probe {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

/// A probe, and the count of the times it has been let go of.
fn probe() -> (Probe, Arc<AtomicUsize>) {
    let freed = Arc::new(AtomicUsize::new(0));
    (Probe(Arc::clone(&freed)), freed)
}

/// `--offline`, and a tracker with no remote, publish nothing: what the write held comes back
/// as it was, never let go of, and nothing is said of publishing.
#[test]
fn with_nowhere_to_publish_what_the_write_held_is_handed_back() {
    let (sandbox, machine) = tracker();
    let ws = workspace(&machine, sandbox.origin(), &[]);
    let (held, freed) = probe();
    let Published::Unsent {
        held,
        warning: None,
    } = publish_first(&ws, NetworkMode::Offline, None, held)
    else {
        panic!("an offline write was published");
    };
    assert_eq!(freed.load(Ordering::SeqCst), 0, "handed back whole");
    drop(held);
    let bare = Sandbox::new(built_bead());
    let alone = Machine::new(&bare, "a");
    git(["remote", "remove", "origin"], Some(alone.checkout()));
    alone.bead(["init", "--prefix", "ex"]);
    let ws = without_origin(&alone, Object::new());
    let (held, freed) = probe();
    let Published::Unsent {
        held,
        warning: None,
    } = publish_first(&ws, NetworkMode::Online, None, held)
    else {
        panic!("a write with no remote was published");
    };
    assert_eq!(freed.load(Ordering::SeqCst), 0, "handed back whole");
    drop(held);
}

/// A remote that cannot be chosen is not exchanged with either: what the write held comes back,
/// and the publish says why, in the words a failed publish says it. Here `.beadroll.json` names
/// one remote where the store already coordinates through another.
#[test]
fn a_remote_that_cannot_be_chosen_hands_it_back_and_says_why() {
    let (sandbox, machine) = tracker();
    let other = sandbox.bare("other.git");
    machine.bead(["sync", "--remote", other.to_str().expect("a path")]);
    let mut config = Object::new();
    let elsewhere = sandbox.root().join("elsewhere.git");
    config.insert(
        "remote".into(),
        Value::from(elsewhere.to_str().expect("a path")),
    );
    let ws = without_origin(&machine, config);
    let (held, freed) = probe();
    let Published::Unsent {
        held,
        warning: Some(warning),
    } = publish_first(&ws, NetworkMode::Online, None, held)
    else {
        panic!("a remote that cannot be chosen was exchanged with, or said nothing");
    };
    assert_eq!(warning, "recorded locally, not yet published (CONFIG)");
    assert_eq!(freed.load(Ordering::SeqCst), 0, "handed back whole");
    drop(held);
}

/// A write with a remote is exchanged with it, and what the write held is let go of by then,
/// once: the publish has it no more.
#[test]
fn an_exchange_lets_go_of_what_the_write_held() {
    let (sandbox, machine) = tracker();
    let ws = workspace(&machine, sandbox.origin(), &[]);
    let (held, freed) = probe();
    let published = publish_first(&ws, NetworkMode::Online, None, held);
    assert!(
        matches!(published, Published::Exchanged { warning: None }),
        "published"
    );
    assert_eq!(freed.load(Ordering::SeqCst), 1, "let go of, once");
}
