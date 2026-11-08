//! Observe unused internal merge projections on real production exits, after their state drops.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use beadroll_core::code::Code;
use beadroll_core::json::{Format, Object, Value, write};
use beadroll_store::{Actor, Endpoint, LockState, Workspace, pending_files};
use beadroll_test_support::json;

use super::Link;
use super::tests::fixture;
use crate::synchronise::SyncReport;
use crate::tx::DropProbe;
use crate::{Error, Mode, NetworkMode, coordinated, locked, synchronise};

struct Observation {
    lock: PathBuf,
    states: Arc<Mutex<Vec<LockState>>>,
}

thread_local! {
    // Only the test's thread and selected workspace are observed; no production state or hooks.
    static OBSERVATION: RefCell<Option<Observation>> = const { RefCell::new(None) };
}

/// Attach only for tests whose merge stays unused: a state moved into Tx has a separate probe.
pub(super) fn probe(ws: &Workspace) -> Option<DropProbe> {
    OBSERVATION.with_borrow(|slot| {
        slot.as_ref()
            .filter(|observation| observation.lock == ws.dir.join("lock"))
            .map(|observation| {
                DropProbe::new(observation.lock.clone(), Arc::clone(&observation.states))
            })
    })
}

struct Observing;

impl Drop for Observing {
    fn drop(&mut self) {
        OBSERVATION.with_borrow_mut(|slot| *slot = None);
    }
}

fn observe<T>(ws: &Workspace, body: impl FnOnce() -> T) -> (T, Vec<LockState>) {
    let states = Arc::new(Mutex::new(Vec::new()));
    OBSERVATION.with_borrow_mut(|slot| {
        assert!(slot.is_none(), "observations must not nest");
        *slot = Some(Observation {
            lock: ws.dir.join("lock"),
            states: Arc::clone(&states),
        });
    });
    let _observing = Observing;
    let result = body();
    let states = states.lock().unwrap().clone();
    (result, states)
}

#[test]
fn unused_merge_probe_detects_destruction_while_locked() {
    let (sandbox, ws) = fixture("");
    let mut link = Link::new(&ws, &Endpoint::at(sandbox.origin())).unwrap();
    let (result, states) = observe(&ws, || {
        locked(&ws, || -> Result<(), Error> {
            let tip = link.fetch()?;
            let merged = link.merge_from(tip.as_deref(), SyncReport::Collect)?;
            drop(merged);
            Ok(())
        })
    });
    result.unwrap();
    assert_eq!(states, [LockState::Locked]);
}

#[test]
fn failed_sync_destroys_its_unused_merge_after_unlocking() {
    let (sandbox, ws) = fixture("publish.result=error(IO)");
    let (result, states) = observe(&ws, || synchronise(&ws, &Endpoint::at(sandbox.origin())));
    assert_eq!(result.unwrap_err().code(), Code::Io);
    assert_eq!(ws.settings.failpoints.hits("publish.result"), 1);
    assert_eq!(states, [LockState::Unlocked]);
}

#[test]
fn accepted_receipt_destroys_its_unused_merge_after_unlocking() {
    let (_sandbox, ws) = fixture("ownership.after-confirm=error(IO)");
    let actor = Actor::writable(json!({"agent": "codex", "thread": "merge-drop"}));
    let signature = write(
        &json!({"actor": (actor.value.clone()), "args": {"command": "claim"}, "request": null}),
        Format::SORTED_ASCII,
    )
    .unwrap();
    let result = coordinated(
        &ws,
        &actor,
        NetworkMode::Online,
        None,
        &signature,
        |tx, mode| -> Result<_, Error> {
            let uid = tx.state()?.issues().next().unwrap().uid().to_string();
            let mut data = Object::new();
            data.insert("mode".into(), Value::from(mode.as_str()));
            data.insert("generation".into(), Value::from(1));
            tx.emit("claim", Some(&uid), data, None)?;
            Ok(mode)
        },
    );
    assert_eq!(result.unwrap_err().code(), Code::Io);
    assert_eq!(ws.settings.failpoints.hits("ownership.after-confirm"), 1);
    assert_eq!(pending_files(&ws).len(), 1);
    let (result, states) = observe(&ws, || {
        coordinated(
            &ws,
            &actor,
            NetworkMode::Online,
            None,
            &signature,
            |_, _| -> Result<Mode, Error> { panic!("accepted receipt must skip the build") },
        )
    });
    assert_eq!(result.unwrap(), Mode::Online);
    assert!(pending_files(&ws).is_empty());
    assert_eq!(states, [LockState::Unlocked]);
}
