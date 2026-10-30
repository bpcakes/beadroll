//! What a run does with what its command held when it answered: the state it answered from, or the
//! transaction a write was made in. Kept until the process exits, so that nobody waits for it to
//! be freed (roll-lqjj.32, roll-lqjj.42); freed first when the run goes on to publish, which reads
//! a state of its own, so that no command holds two.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use beadroll_core::json::{Object, Value};
use beadroll_store::PublicationState;
use beadroll_store::Workspace;
use beadroll_sync::NetworkMode;
use beadroll_sync::Spent;
use beadroll_test_support::{Machine, Sandbox, built_bead, git};

use super::{Answer, Failure, Held, Line, Outcome, Tone, ended_with, finish};
use crate::cli::AfterCommand;
use crate::cli::RunLifetime;
use crate::testing::{settings, tracker, workspace};

/// Something a command held, which counts how often it was freed.
#[derive(Debug)]
struct Probe(Arc<AtomicUsize>);

impl Held for Probe {}

impl Drop for Probe {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

/// An outcome that holds a probe, and the count of its being freed.
fn holding_a_probe() -> (Outcome, Arc<AtomicUsize>) {
    let freed = Arc::new(AtomicUsize::new(0));
    let mut outcome = Outcome::new(Value::Null, Vec::<String>::new());
    outcome.held = Some(Box::new(Probe(Arc::clone(&freed))));
    (outcome, freed)
}

/// How often the probe was freed by the time `ended_with` returned, and whether it said it kept it.
fn ending(after_command: AfterCommand, run_lifetime: RunLifetime) -> (bool, usize) {
    let (outcome, freed) = holding_a_probe();
    let kept = ended_with(outcome.held, after_command, run_lifetime);
    (kept, freed.load(Ordering::SeqCst))
}

/// Kept for good (never freed, so the count stays at nought) only by a process that ends with the
/// run and publishes nothing; freed by the time the call returns in every other case, so before
/// whatever the run does next.
#[test]
fn what_a_command_held_is_kept_only_by_a_process_that_ends_with_it_and_publishes_nothing() {
    assert_eq!(
        ending(AfterCommand::Return, RunLifetime::Process),
        (true, 0)
    );
    assert_eq!(
        ending(AfterCommand::Publish, RunLifetime::Process),
        (false, 1)
    );
    assert_eq!(
        ending(AfterCommand::Return, RunLifetime::Embedded),
        (false, 1)
    );
    assert_eq!(
        ending(AfterCommand::Publish, RunLifetime::Embedded),
        (false, 1)
    );
    assert!(!ended_with(
        None,
        crate::cli::AfterCommand::Return,
        crate::cli::RunLifetime::Process
    ));
    // a transaction is held as a state is
    let spent: Option<Box<dyn Held>> = Some(Box::new(Spent::default()));
    assert!(ended_with(
        spent,
        crate::cli::AfterCommand::Return,
        crate::cli::RunLifetime::Process
    ));
}

/// Runs `finish` on an outcome holding a probe, for a command that wrote (`wrote`) or did not, as
/// the process ends with it when `exits`: what it answered, and how often the probe was freed.
fn finishing(
    ws: &Workspace,
    publication_state: PublicationState,
    network_mode: NetworkMode,
    run_lifetime: RunLifetime,
) -> (Result<Option<Answer>, Failure>, Arc<AtomicUsize>) {
    let (outcome, freed) = holding_a_probe();
    ws.set_wrote(publication_state);
    (
        finish(outcome, Some(ws), network_mode, &None, run_lifetime, |_| {}),
        freed,
    )
}

/// A write that is not published (`--offline`) keeps what it held past `finish`, to the end of the
/// process; one that goes on in its process lets go of it, and it is at the same place.
#[test]
fn a_write_that_is_not_published_keeps_what_it_held_when_the_process_ends_with_it() {
    let (sandbox, machine) = tracker();
    let ws = workspace(&machine, sandbox.origin(), &[]);
    let (answer, freed) = finishing(
        &ws,
        PublicationState::Unpublished,
        NetworkMode::Offline,
        RunLifetime::Process,
    );
    assert!(answer.is_ok_and(|answer| answer.is_some()));
    assert_eq!(freed.load(Ordering::SeqCst), 0, "kept: the process ends");
    let (answer, freed) = finishing(
        &ws,
        PublicationState::Unpublished,
        NetworkMode::Offline,
        RunLifetime::Embedded,
    );
    assert!(answer.is_ok_and(|answer| answer.is_some()));
    assert_eq!(
        freed.load(Ordering::SeqCst),
        1,
        "freed: the process goes on"
    );
}

/// What a run publishes is what its command wrote, not whatever is not `--offline`: a read, which
/// wrote nothing and is not `--offline` either, publishes nothing, so it keeps the state it
/// answered from to the end of the process (roll-lqjj.32). Were the flag alone to decide, every read
/// would free its state first, and the gain of keeping it would be lost.
#[test]
fn a_command_that_wrote_nothing_keeps_what_it_held_though_it_is_not_offline() {
    let (sandbox, machine) = tracker();
    let ws = workspace(&machine, sandbox.origin(), &[]);
    let (answer, freed) = finishing(
        &ws,
        PublicationState::Published,
        NetworkMode::Online,
        RunLifetime::Process,
    );
    assert!(answer.is_ok_and(|answer| answer.is_some()));
    assert_eq!(freed.load(Ordering::SeqCst), 0, "kept: nothing to publish");
    let (answer, freed) = finishing(
        &ws,
        PublicationState::Published,
        NetworkMode::Online,
        RunLifetime::Embedded,
    );
    assert!(answer.is_ok_and(|answer| answer.is_some()));
    assert_eq!(
        freed.load(Ordering::SeqCst),
        1,
        "freed: the process goes on"
    );
}

/// Opens the machine's store as a checkout without an origin opens it, with `config` as what its
/// `.beadroll.json` tracks.
fn without_origin(machine: &Machine, config: Object) -> Workspace {
    let store = machine.store();
    let id = store
        .file_name()
        .and_then(OsStr::to_str)
        .expect("a store id");
    Workspace::new(settings(machine, &[]), id, None, None, config).expect("the store")
}

/// A write on a tracker with no remote publishes nothing, as an offline write publishes nothing,
/// so it keeps what it held to the end of the process as that write does, and says nothing of
/// publishing (roll-lqjj.50). Were `--offline` alone to decide, it would free its transaction
/// first for a publish with nowhere to go: about 83 ms at 200,000 events.
#[test]
fn a_write_on_a_tracker_with_no_remote_keeps_what_it_held_when_the_process_ends_with_it() {
    let sandbox = Sandbox::new(built_bead());
    let machine = Machine::new(&sandbox, "a");
    git(["remote", "remove", "origin"], Some(machine.checkout()));
    machine.bead(["init", "--prefix", "ex"]);
    let ws = without_origin(&machine, Object::new());
    assert!(
        matches!(ws.endpoint(None), Ok(None)),
        "a tracker with no remote"
    );
    let (answer, freed) = finishing(
        &ws,
        PublicationState::Unpublished,
        NetworkMode::Online,
        RunLifetime::Process,
    );
    let (_, lines) = answer.expect("finished").expect("answered");
    assert_eq!(freed.load(Ordering::SeqCst), 0, "kept: the process ends");
    assert_eq!(lines, Some(Vec::new()), "nothing to say of publishing");
    let (answer, freed) = finishing(
        &ws,
        PublicationState::Unpublished,
        NetworkMode::Online,
        RunLifetime::Embedded,
    );
    assert!(answer.is_ok_and(|answer| answer.is_some()));
    assert_eq!(
        freed.load(Ordering::SeqCst),
        1,
        "freed: the process goes on"
    );
}

/// A write whose remote cannot be chosen publishes nothing either, so it keeps what it held; and
/// it says why where a publish that failed says it, in the same words: here `.beadroll.json`
/// names one remote where the store already coordinates through another.
#[test]
fn a_write_whose_remote_cannot_be_chosen_keeps_what_it_held_and_says_why() {
    let (sandbox, machine) = tracker();
    let other = sandbox.bare("other.git");
    machine.bead(["sync", "--remote", other.to_str().expect("a path")]);
    let elsewhere = sandbox.root().join("elsewhere.git");
    let mut config = Object::new();
    let elsewhere = elsewhere.to_str().expect("a path");
    config.insert("remote".into(), Value::from(elsewhere));
    let ws = without_origin(&machine, config);
    assert!(ws.endpoint(None).is_err(), "two remotes chosen");
    let (answer, freed) = finishing(
        &ws,
        PublicationState::Unpublished,
        NetworkMode::Online,
        RunLifetime::Process,
    );
    let (_, lines) = answer.expect("finished").expect("answered");
    assert_eq!(
        freed.load(Ordering::SeqCst),
        0,
        "kept: nothing is published"
    );
    let said = Line::toned(
        Tone::Warning,
        "recorded locally, not yet published (CONFIG)",
    );
    assert_eq!(lines, Some(vec![said]));
}

/// A write given `--remote` is published there, not to the tracker's own remote, and that choice
/// decides the free as well: the transaction is let go of first, and a remote that does not answer
/// leaves the write unpublished and says so, on a tracker with a remote and on one with none.
#[test]
fn a_write_given_a_remote_is_published_there_and_lets_go_of_what_it_held_first() {
    let (sandbox, machine) = tracker();
    let nowhere = Some(sandbox.root().join("nowhere.git").into_os_string());
    let said = Line::toned(
        Tone::Warning,
        "recorded locally, not yet published (REMOTE_UNREACHABLE)",
    );
    let with_origin = workspace(&machine, sandbox.origin(), &[]);
    for ws in [with_origin, without_origin(&machine, Object::new())] {
        let (outcome, freed) = holding_a_probe();
        ws.set_wrote(PublicationState::Unpublished);
        let answer = finish(
            outcome,
            Some(&ws),
            NetworkMode::Online,
            &nowhere,
            RunLifetime::Process,
            |_| {},
        );
        let (_, lines) = answer.expect("finished").expect("answered");
        assert_eq!(
            freed.load(Ordering::SeqCst),
            1,
            "freed: a remote was chosen"
        );
        assert_eq!(lines, Some(vec![said.clone()]));
    }
}

/// The handshake of a `pause-until` failpoint (`crates/beadroll-store/src/failpoint.rs`): the
/// paused run touches `PATH.ready`, then waits for `PATH.go`. Once it has reached the gate (or
/// `done` says it will not), runs `step`, then opens the gate, whatever `step` came to.
fn meet<T>(gate: &Path, done: &dyn Fn() -> bool, step: impl FnOnce() -> T) -> Option<T> {
    struct Open(PathBuf);
    impl Drop for Open {
        fn drop(&mut self) {
            let _ = std::fs::write(&self.0, b"");
        }
    }
    let with = |suffix: &str| {
        let mut name = gate.as_os_str().to_owned();
        name.push(suffix);
        PathBuf::from(name)
    };
    let _open = Open(with(".go"));
    let deadline = Instant::now() + Duration::from_secs(120);
    while !with(".ready").exists() {
        if done() || Instant::now() > deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    Some(step())
}

/// A write that is published at once lets go of what it held before the publish begins to read the
/// store's state, which is what the publish needs a projection of: at the failpoint where that
/// read starts, the run is stopped and the probe has been freed already, once. Were it kept to the
/// end of the process, this publish would hold the write's transaction beside its own.
#[test]
fn a_write_that_publishes_lets_go_of_what_it_held_before_the_publish_reads_a_state() {
    let (sandbox, machine) = tracker();
    let gate = sandbox.root().join("publish-reads");
    let held = format!("projection.read=pause-until({})", gate.display());
    let ws = workspace(&machine, sandbox.origin(), &[("BEAD_FAILPOINTS", &held)]);
    let (outcome, freed) = holding_a_probe();
    ws.set_wrote(PublicationState::Unpublished);
    let finished = AtomicUsize::new(0);
    let (answer, met) = std::thread::scope(|scope| {
        let meeting = scope.spawn(|| {
            let done = || finished.load(Ordering::SeqCst) > 0;
            meet(&gate, &done, || freed.load(Ordering::SeqCst))
        });
        let answer = finish(
            outcome,
            Some(&ws),
            NetworkMode::Online,
            &None,
            RunLifetime::Process,
            |_| {},
        );
        finished.store(1, Ordering::SeqCst);
        let met = meeting
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
        (answer, met)
    });
    assert_eq!(met, Some(1), "freed by the time the publish read a state");
    assert_eq!(ws.settings.failpoints.hits("projection.read"), 1);
    assert!(answer.is_ok_and(|answer| answer.is_some()));
    assert_eq!(freed.load(Ordering::SeqCst), 1, "and not again");
}
