//! Observe the real store lock when a failed write destroys its materialized view. A probe is
//! attached after the body's final state read, and follows that view through preparation.

use std::fs::{File, TryLockError};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use beadroll_core::code::Code;
use beadroll_core::json::{Object, Value};
use beadroll_store::{Actor, LockState, Settings, Workspace};
use beadroll_test_support::{Machine, Sandbox, built_bead, json};

use super::materialized::Materialized;
use super::{Spent, Tx, locked, writing};
use crate::{Error, Mode, NetworkMode, coordinated};

/// The enum drops the view before this field, so this records its actual destruction boundary.
#[derive(Debug)]
pub(super) struct DropProbe {
    lock: PathBuf,
    observed: Arc<Mutex<Vec<LockState>>>,
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
                tx.emit("claim", Some(&uid), data, None)?;
                observe(tx, view, &observed);
                Ok(mode)
            });
            assert_eq!(result.expect_err("persistence failed").code(), Code::Io);
            assert_eq!(fixture.ws.settings.failpoints.hits(point), 1);
            assert_eq!(*observed.lock().unwrap(), [LockState::Unlocked]);
        }
    }
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
        tx.emit("comment", Some(&uid), comment(), None)?;
        {
            let mut attempt = tx.attempt();
            attempt.emit("comment", Some(&uid), comment(), None)?;
            let (_, mut emitter) = attempt.lend()?;
            emitter.emit("comment", Some(&uid), comment(), None)?;
            assert!(
                emitter
                    .emit("not-an-operation", Some(&uid), Object::new(), None)
                    .is_err()
            );
        }
        tx.emit("comment", Some(&uid), comment(), None)?;
        assert_eq!(
            tx.emitted
                .iter()
                .map(|record| record.sequenced)
                .collect::<Vec<_>>(),
            [true, false, false, true]
        );
        assert_eq!(
            tx.ownership,
            [tx.emitted[1].value.clone(), tx.emitted[2].value.clone()]
        );
        assert!(!tx.conditional);
        Ok(())
    })
    .unwrap();
}

/// Legacy callers may keep conditional routing enabled before and after an attempt scope.
#[test]
fn legacy_conditional_routing_survives_an_attempt_scope() {
    let fixture = Fixture::new("");
    locked(&fixture.ws, || -> Result<(), Error> {
        let mut tx = Tx::new(&fixture.ws, &fixture.actor)?;
        let uid = tx.state()?.resolve(&fixture.key).unwrap().uid().to_string();
        let comment = || {
            let mut data = Object::new();
            data.insert("text".into(), Value::from("legacy comment"));
            data
        };
        tx.conditional = true;
        tx.emit("comment", Some(&uid), comment(), None)?;
        tx.lend()?.1.emit("comment", Some(&uid), comment(), None)?;
        {
            let mut attempt = tx.attempt();
            attempt.emit("comment", Some(&uid), comment(), None)?;
            attempt
                .lend()?
                .1
                .emit("comment", Some(&uid), comment(), None)?;
        }
        tx.emit("comment", Some(&uid), comment(), None)?;
        tx.lend()?.1.emit("comment", Some(&uid), comment(), None)?;
        tx.conditional = false;
        tx.emit("comment", Some(&uid), comment(), None)?;
        tx.lend()?.1.emit("comment", Some(&uid), comment(), None)?;
        assert_eq!(
            tx.emitted
                .iter()
                .map(|record| record.sequenced)
                .collect::<Vec<_>>(),
            [false, false, false, false, false, false, true, true]
        );
        assert_eq!(
            tx.ownership,
            tx.emitted[..6]
                .iter()
                .map(|record| record.value.clone())
                .collect::<Vec<_>>()
        );
        Ok(())
    })
    .unwrap();
}
