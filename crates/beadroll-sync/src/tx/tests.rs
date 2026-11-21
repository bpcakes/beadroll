//! Observe the real store lock when a failed write destroys its materialized view. A probe is
//! attached after the body's final state read, and follows that view through preparation.

use std::fs::{File, TryLockError};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use beadroll_core::code::Code;
use beadroll_core::json::{Object, Value};
use beadroll_core::op::OpName;
use beadroll_store::{Actor, LockState, Settings, Workspace};
use beadroll_test_support::{Machine, Sandbox, built_bead, json};

use super::materialized::Materialized;
use super::{Spent, Tx, locked, writing};
use crate::{Error, Mode, NetworkMode, coordinated};

/// The enum drops the view before this field, so this records its actual destruction boundary.
#[derive(Debug)]
pub(crate) struct DropProbe {
    lock: PathBuf,
    observed: Arc<Mutex<Vec<LockState>>>,
}

impl DropProbe {
    pub(crate) fn new(lock: PathBuf, observed: Arc<Mutex<Vec<LockState>>>) -> Self {
        Self { lock, observed }
    }
}

impl Drop for DropProbe {
    fn drop(&mut self) {
        let file = File::options()
            .read(true)
            .write(true)
            .open(&self.lock)
            .expect("the existing store lock");
        let state = match file.try_lock() {
            Ok(()) => LockState::Unlocked,
            Err(TryLockError::WouldBlock) => LockState::Locked,
            Err(TryLockError::Error(error)) => panic!("probe the store lock: {error}"),
        };
        self.observed.lock().expect("observations").push(state);
    }
}

impl Spent {
    /// Observe this retained view's real destruction, for compatibility merge tests too.
    pub(crate) fn observe_drop(&mut self, lock: PathBuf) -> Arc<Mutex<Vec<LockState>>> {
        let observed = Arc::new(Mutex::new(Vec::new()));
        let slot = match &mut self.materialized {
            Materialized::Projected { drop_probe, .. }
            | Materialized::Replayed { drop_probe, .. } => drop_probe,
            Materialized::ReplayNeeded => panic!("no retained view to observe"),
        };
        *slot = Some(DropProbe {
            lock,
            observed: Arc::clone(&observed),
        });
        observed
    }
}

#[derive(Clone, Copy, Debug)]
enum View {
    Projection,
    Replay,
}

#[derive(Clone, Copy, Debug)]
enum Write {
    Local,
    Coordinated,
}

struct Fixture {
    ws: Workspace,
    actor: Actor,
    key: String,
    _sandbox: Sandbox,
}

impl Fixture {
    fn new(fault: &str) -> Self {
        let sandbox = Sandbox::new(built_bead());
        let machine = Machine::new(&sandbox, "a").with_bead(built_bead());
        machine.bead(["init", "--prefix", "ex"]);
        let key = machine.bead(["create", "x"]).expect("an issue")["id"]
            .as_str()
            .expect("an issue key")
            .to_owned();
        let mut env = machine.env(None, "codex");
        env.insert("BEAD_FAILPOINTS".into(), fault.into());
        let settings = Settings::from_process(
            env,
            Some(machine.checkout().to_path_buf()),
            std::env::var_os("HOME").unwrap_or_default(),
            std::env::temp_dir(),
            &|_| None,
        )
        .expect("the machine's settings");
        let store = machine.store();
        let id = store
            .file_name()
            .and_then(|s| s.to_str())
            .expect("store id");
        let ws = Workspace::new(
            Arc::new(settings),
            id,
            None,
            Some(sandbox.origin().as_os_str().to_owned()),
            Object::new(),
        )
        .expect("the machine's workspace");
        Self {
            ws,
            actor: Actor::writable(json!({"agent": "codex", "thread": "drop-order"})),
            key,
            _sandbox: sandbox,
        }
    }

    fn write(
        &self,
        write: Write,
        mut body: impl FnMut(&mut Tx<'_>, Mode) -> Result<Mode, Error>,
    ) -> Result<Mode, Error> {
        match write {
            Write::Local => writing(&self.ws, &self.actor, |tx| body(tx, Mode::Local)),
            Write::Coordinated => coordinated(
                &self.ws,
                &self.actor,
                NetworkMode::Online,
                None,
                "drop-order",
                |tx, mode| {
                    assert_eq!(mode, Mode::Online, "exercise the coordinated lock");
                    body(tx, mode)
                },
            ),
        }
    }
}

/// Attach only after the last application: replacement during application is a different lifetime.
fn observe(tx: &mut Tx<'_>, view: View, observed: &Arc<Mutex<Vec<LockState>>>) {
    if matches!(view, View::Replay) {
        tx.materialized = Materialized::ReplayNeeded;
    }
    tx.state().expect("a materialized view");
    let slot = match (&mut tx.materialized, view) {
        (Materialized::Projected { drop_probe, .. }, View::Projection)
        | (Materialized::Replayed { drop_probe, .. }, View::Replay) => drop_probe,
        _ => panic!("the requested view was not materialized"),
    };
    *slot = Some(DropProbe {
        lock: tx.ws.dir.join("lock"),
        observed: Arc::clone(observed),
    });
}

#[test]
fn a_drop_probe_detects_a_view_destroyed_while_locked() {
    let fixture = Fixture::new("");
    for view in [View::Projection, View::Replay] {
        let observed = Arc::new(Mutex::new(Vec::new()));
        locked(&fixture.ws, || -> Result<(), Error> {
            let mut tx = Tx::new(&fixture.ws, &fixture.actor)?;
            observe(&mut tx, view, &observed);
            drop(tx);
            Ok(())
        })
        .expect("the locked control");
        assert_eq!(*observed.lock().unwrap(), [LockState::Locked]);
    }
}

#[test]
fn failed_builds_destroy_the_view_after_unlocking() {
    for write in [Write::Local, Write::Coordinated] {
        let fixture = Fixture::new("");
        for view in [View::Projection, View::Replay] {
            let observed = Arc::new(Mutex::new(Vec::new()));
            let result = fixture.write(write, |tx, _| {
                observe(tx, view, &observed);
                Err(Error::Validation("body refused".to_owned()))
            });
            assert_eq!(result, Err(Error::Validation("body refused".to_owned())));
            assert_eq!(*observed.lock().unwrap(), [LockState::Unlocked]);
        }
    }
}

#[test]
fn failed_persistence_destroys_the_view_after_unlocking() {
    for write in [Write::Local, Write::Coordinated] {
        let point = match write {
            Write::Local => "atomic-write:claims/",
            Write::Coordinated => "atomic-write:pending/",
        };
        for view in [View::Projection, View::Replay] {
            let fixture = Fixture::new(&format!("{point}=errno(ENOSPC)"));
            let observed = Arc::new(Mutex::new(Vec::new()));
            let result = fixture.write(write, |tx, mode| {
                let issue = tx.state()?.resolve(&fixture.key).expect("the issue");
                let uid = issue.uid().to_string();
                let mut data = Object::new();
                data.insert("mode".into(), Value::from(mode.as_str()));
                data.insert("generation".into(), Value::Int(1.into()));
                tx.emit(OpName::Claim, Some(&uid), data, None)?;
                observe(tx, view, &observed);
                Ok(mode)
            });
            assert_eq!(result.expect_err("persistence failed").code(), Code::Io);
            assert_eq!(fixture.ws.settings.failpoints.hits(point), 1);
            assert_eq!(*observed.lock().unwrap(), [LockState::Unlocked]);
        }
    }
}

/// An event is written only within the 100 levels every reader takes back: its object, its data,
/// then what the data holds. A member 98 arrays deep is written; one 99 deep is `Validation`, and
/// nothing is recorded for it.
#[test]
fn an_event_nested_past_a_hundred_levels_is_refused() {
    let fixture = Fixture::new("");
    let nested = |depth: usize| {
        (0..depth).fold(Value::Array(Vec::new()), |inner, _| {
            Value::Array(vec![inner])
        })
    };
    locked(&fixture.ws, || -> Result<(), Error> {
        let mut tx = Tx::new(&fixture.ws, &fixture.actor)?;
        let uid = tx.state()?.resolve(&fixture.key).unwrap().uid().to_string();
        let comment = |depth: usize| {
            let mut data = Object::new();
            data.insert("text".into(), Value::from("deep"));
            data.insert("x_deep".into(), nested(depth - 1));
            data
        };
        tx.emit(OpName::Comment, Some(&uid), comment(98), None)?;
        let refused = tx.emit(OpName::Comment, Some(&uid), comment(99), None);
        match refused {
            Err(Error::Validation(message)) => {
                assert!(message.contains("more than 100 levels"), "{message}");
            }
            other => panic!("not refused as too deep: {other:?}"),
        }
        assert_eq!(tx.emitted.len(), 1, "the refused event was recorded");
        Ok(())
    })
    .unwrap();
}

/// An attempt scope routes ordinary edits with the claim and never changes later emission.
#[test]
fn attempt_emission_is_scoped_even_after_an_error() {
    let fixture = Fixture::new("");
    locked(&fixture.ws, || -> Result<(), Error> {
        let mut tx = Tx::new(&fixture.ws, &fixture.actor)?;
        let uid = tx.state()?.resolve(&fixture.key).unwrap().uid().to_string();
        let comment = || {
            let mut data = Object::new();
            data.insert("text".into(), Value::from("comment"));
            data
        };
        tx.emit(OpName::Comment, Some(&uid), comment(), None)?;
        {
            let mut attempt = tx.attempt();
            attempt.emit(OpName::Comment, Some(&uid), comment(), None)?;
            let (_, mut emitter) = attempt.lend()?;
            emitter.emit(OpName::Comment, Some(&uid), comment(), None)?;
            // a refused event records nothing: a comment whose text is not text
            let mut malformed = Object::new();
            malformed.insert("text".into(), Value::from(1));
            assert!(
                emitter
                    .emit(OpName::Comment, Some(&uid), malformed, None)
                    .is_err()
            );
        }
        // emitted as before the scope, by the journal's route, which the attempt begun refuses
        let after = tx.emit(OpName::Comment, Some(&uid), comment(), None);
        assert!(matches!(after, Err(Error::Validation(_))), "{after:?}");
        assert_eq!(
            tx.emitted
                .iter()
                .map(|record| record.sequenced)
                .collect::<Vec<_>>(),
            [true, false, false]
        );
        assert_eq!(
            tx.ownership,
            [tx.emitted[1].value.clone(), tx.emitted[2].value.clone()]
        );
        Ok(())
    })
    .unwrap();
}

/// A create the transaction emitted is a parent by its record only while the state applies fewer
/// events than make it: once it does, the state alone says, and a create replay set aside (a
/// broken chain under a foreign pending cycle) is no parent (AT100).
#[test]
fn a_create_the_state_applies_is_a_parent_only_if_replay_made_it() {
    use std::collections::HashMap;

    use beadroll_core::id::EventId;
    use beadroll_core::journal::Journal;

    use super::{Made, parent_problem};

    let state = beadroll_core::replay::replay(Journal::new()).expect("an empty state");
    let made = EventId::parse("0190a8a2-5e9a-7000-8000-000000000001").expect("an id");
    let created = HashMap::from([(made, 3)]);
    let mut data = Object::new();
    data.insert("parent".into(), Value::from(made.to_string()));
    let judged = |applied| {
        let made = Made {
            state: Some(&state),
            applied,
        };
        parent_problem(made, &created, OpName::Create, &data)
    };
    // the fourth event emitted, not yet in the state: by the transaction's record
    assert!(judged(3).is_ok());
    // applied, and no issue of the state: replay set it aside
    assert!(matches!(judged(4), Err(Error::Validation(_))));
    let none = Made {
        state: None,
        applied: 0,
    };
    assert!(parent_problem(none, &created, OpName::Create, &data).is_ok());
}

/// Every record of the journal comes before the attempt's first event, which builds on them: the
/// journal is appended before the attempt is published, and a record after it, built on its events,
/// waited for them when the remote refused the attempt, or, built beside them, could replay before
/// the claim, edit or create of the attempt it relied on, its effect lost though both landed. Any
/// record of the journal after the attempt began is refused, whether or not the state was read
/// between, and goes through the attempt instead (AT104).
#[test]
fn a_record_of_the_journal_after_the_attempt_began_is_refused() {
    let fixture = Fixture::new("");
    locked(&fixture.ws, || -> Result<(), Error> {
        let mut tx = Tx::new(&fixture.ws, &fixture.actor)?;
        let uid = tx.state()?.resolve(&fixture.key).unwrap().uid().to_string();
        let comment = || {
            let mut data = Object::new();
            data.insert("text".into(), Value::from("comment"));
            data
        };
        let labels = || {
            let mut data = Object::new();
            data.insert("labels".into(), Value::Array(vec![Value::from("ui")]));
            data
        };
        let journal = tx.emit(OpName::Comment, Some(&uid), comment(), None)?;
        let attempted = tx.attempt().emit(OpName::LabelAdd, Some(&uid), labels(), None)?;
        let deps = |tx: &Tx<'_>, at: usize| -> Vec<String> {
            match &tx.emitted[at].value["deps"] {
                Value::Array(deps) => deps.iter().map(|dep| dep.as_str().unwrap().into()).collect(),
                _ => Vec::new(),
            }
        };
        assert_eq!(deps(&tx, 1), [journal.to_string()], "the attempt on the journal");
        let refused = |judged: Result<_, Error>| {
            matches!(&judged, Err(Error::Validation(problem)) if problem.contains("attempt"))
        };
        for read_between in [false, true] {
            if read_between {
                tx.state()?;
            }
            // undoing what the attempt did, on an issue the attempt is not about, or a claim
            let undone = tx.emit(OpName::LabelRemove, Some(&uid), labels(), None);
            assert!(refused(undone), "read between: {read_between}");
            let elsewhere = tx.emit(OpName::Comment, None, comment(), None);
            assert!(refused(elsewhere), "read between: {read_between}");
            let mut claim = Object::new();
            claim.insert("mode".into(), Value::from("online"));
            claim.insert("generation".into(), Value::from(1));
            assert!(tx.emit(OpName::Claim, Some(&uid), claim, None).is_ok(), "an attempt's");
        }
        assert_eq!(tx.emitted.len(), 4, "nothing refused recorded");
        let after = tx.attempt().emit(OpName::LabelRemove, Some(&uid), labels(), None)?;
        assert_eq!(deps(&tx, 4), [tx.emitted[3].value["id"].as_str().unwrap().to_owned()]);
        assert!(after != attempted);
        Ok(())
    })
    .unwrap();
}
