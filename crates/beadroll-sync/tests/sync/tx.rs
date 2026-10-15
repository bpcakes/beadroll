//! A transaction's state while it emits ([`Tx::lend`], roll-lqjj.29): what is lent is the state
//! before, and the next state asked for has every event emitted since, from the projection or
//! from the journal replayed.

use beadroll_core::id::EventId;
use beadroll_core::json::{Object, Value};
use beadroll_sync::{Error, Tx, locked};
use beadroll_test_support::Machine;

use crate::support::{actor, sandbox, text, workspace};

/// `{"fields": {"title": title}}`.
fn titled(title: &str) -> Object {
    let mut fields = Object::new();
    fields.insert("title".into(), Value::from(title));
    let mut data = Object::new();
    data.insert("fields".into(), Value::from(fields));
    data
}

/// The title of the issue `uid` in the transaction's state now.
fn title(tx: &mut Tx<'_>, uid: EventId) -> String {
    let st = tx.state().expect("the state");
    st.issue(uid).expect("the issue").title().to_owned()
}

/// On a machine's first record, which the projection cannot take, the state is the journal
/// replayed; each later emit is in the next state asked for, as on the projection, and what is
/// lent while emitting is the state before.
#[test]
fn a_lent_state_is_the_one_before_and_the_next_has_every_event() {
    let sandbox = sandbox();
    let (a, b) = (Machine::new(&sandbox, "a"), Machine::new(&sandbox, "b"));
    a.bead(["init", "--prefix", "ex"]);
    let key = text(&a.bead(["create", "x"]).expect("an issue")["id"]);
    a.bead(["create", "y"]);
    for machine in [&a, &b] {
        machine.bead(["sync"]);
        let ws = workspace(machine, sandbox.origin(), &[]);
        let actor = actor(machine);
        locked(&ws, || -> Result<(), Error> {
            let mut tx = Tx::new(&ws, &actor)?;
            let uid = tx.state()?.resolve(&key).expect("the issue").uid();
            let id = uid.to_string();
            for name in ["one", "two"] {
                tx.emit("set", Some(&id), titled(name), None)?;
                assert_eq!(title(&mut tx, uid), name);
            }
            {
                let (st, mut emitter) = tx.lend()?;
                emitter.emit("set", Some(&id), titled("three"), None)?;
                emitter.emit("set", Some(&id), titled("four"), None)?;
                assert_eq!(st.issue(uid).expect("the issue").title(), "two");
            }
            assert_eq!(title(&mut tx, uid), "four");
            let state = tx.into_state()?;
            assert_eq!(state.issue(uid).expect("the issue").title(), "four");
            Ok(())
        })
        .expect("a transaction");
    }
}
