//! What a link keeps of git (`crates/beadroll-git/AGENTS.md`): the long-lived gits it asks end with
//! it, killed and reaped, on every path out, an error's included.

use beadroll_store::{Endpoint, REMOTE_REF};
use beadroll_sync::{Error, Link, PublishMode, PushOutcome, locked, synchronise};
use beadroll_test_support::Machine;
use rustix::io::Errno;
use rustix::process::{WaitOptions, wait};

use crate::support::{sandbox, workspace};

/// How many descriptors this process has open.
fn open_descriptors() -> Option<usize> {
    std::fs::read_dir("/dev/fd").ok().map(Iterator::count)
}

/// Syncs through links that went through, and through one an unreachable remote ended, leave this
/// process no child, live or zombie, and no descriptor more than before.
#[test]
fn the_gits_a_link_keeps_end_with_it() {
    let sandbox = sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    a.bead(["create", "x"]);
    a.bead(["sync"]);
    let ws = workspace(&a, sandbox.origin(), &[]);
    let origin = Endpoint::at(sandbox.origin());
    synchronise(&ws, &origin).expect("a first sync");
    let before = open_descriptors();
    for _ in 0..3 {
        synchronise(&ws, &origin).expect("a sync");
    }
    let nowhere = sandbox.root().join("nowhere.git");
    let error = synchronise(&ws, &Endpoint::at(&nowhere)).expect_err("no remote there");
    assert_eq!(error.code().as_str(), "REMOTE_UNREACHABLE", "{error}");
    // Ask the kernel whether we own any child, without a process-list snapshot or a helper to
    // filter out. A live child gives Ok(None), an unreaped exit Ok(Some(_)); only ECHILD passes.
    let children = wait(WaitOptions::NOHANG);
    assert!(matches!(children, Err(Errno::CHILD)), "{children:?}");
    assert_eq!(open_descriptors(), before);
}

/// A link keeps each fetched head reachable, removes its temporary ref, and can use the same
/// ref writer for the next fetch. A failed bookkeeping transaction still has the established
/// best-effort fallback: a locked remote ref does not turn a successful fetch into a failure.
#[test]
fn fetched_heads_are_kept_and_temporary_refs_removed_even_after_a_failed_transaction() {
    let sandbox = sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let ws = workspace(&a, sandbox.origin(), &[]);
    let mut link = Link::new(&ws, &Endpoint::at(sandbox.origin())).expect("a link");
    for n in 0..2 {
        a.bead(["create", &format!("issue {n}")]);
        let tip = link.fetch().expect("a fetch").expect("a head");
        let kept = link
            .env
            .git(["rev-parse", REMOTE_REF])
            .string()
            .expect("kept");
        assert_eq!(kept, tip);
        let temporary = link
            .env
            .git(["for-each-ref", "refs/beads/fetch/"])
            .string()
            .expect("temporary refs");
        assert!(temporary.is_empty(), "{temporary}");
    }
    let lock = link.git_dir.join("refs/beads/remote.lock");
    std::fs::write(&lock, b"").expect("block bookkeeping");
    assert!(link.fetch().expect("bookkeeping is best effort").is_some());
    assert!(
        link.env
            .git(["for-each-ref", "refs/beads/fetch/"])
            .string()
            .expect("temporary refs")
            .is_empty()
    );
    std::fs::remove_file(lock).expect("unblock bookkeeping");
    assert!(link.fetch().expect("a fresh ref writer").is_some());
}

/// Replacing the repository must end every old helper even while the Link remains alive. The
/// same Link can then fetch and publish through new helpers without retaining children or FDs.
#[test]
fn rebuilding_a_live_link_ends_its_helpers_and_it_can_publish_again() {
    let sandbox = sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    a.bead(["create", "x"]);
    let ws = workspace(&a, sandbox.origin(), &[]);
    let mut link = Link::new(&ws, &Endpoint::at(sandbox.origin())).unwrap();
    let before = open_descriptors();
    let tip = link.fetch().unwrap();
    locked(&ws, || -> Result<(), Error> {
        link.merge(tip.as_deref())?;
        assert_eq!(
            link.publish(tip.as_deref(), "sync", &[], PublishMode::Fence)?,
            PushOutcome::Ok
        );
        Ok(())
    })
    .unwrap();
    // Warm the next tree batch as well as the reader, hasher and reused ref writer.
    let published = link.fetch().unwrap();
    assert!(matches!(wait(WaitOptions::NOHANG), Ok(None)));
    locked(&ws, || link.rebuild()).unwrap();
    let children = wait(WaitOptions::NOHANG);
    assert!(matches!(children, Err(Errno::CHILD)), "{children:?}");
    assert_eq!(open_descriptors(), before);
    let tip = link.fetch().unwrap();
    assert_eq!(tip, published);
    locked(&ws, || {
        link.publish(tip.as_deref(), "sync", &[], PublishMode::Fence)
    })
    .unwrap();
    assert_ne!(link.last_commit, published);
    drop(link);
    let children = wait(WaitOptions::NOHANG);
    assert!(matches!(children, Err(Errno::CHILD)), "{children:?}");
    assert_eq!(open_descriptors(), before);
}
