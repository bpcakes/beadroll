//! A transaction's state while it emits ([`Tx::lend`], roll-lqjj.29): what is lent is the state
//! before, and the next state asked for has every event emitted since, from the projection or
//! from the journal replayed; and what a write does with its transaction once it is done
//! ([`writing`] lets go of it, [`writing_spent`] hands it over, roll-lqjj.42).

use beadroll_core::id::EventId;
use beadroll_core::json::{Object, Value};
use beadroll_sync::{Error, Tx, locked, writing, writing_spent};
use beadroll_test_support::Machine;

use crate::support::{actor, comments, sandbox, text, workspace};

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

/// Emits a comment on the issue `key` names and says its text.
fn comment_on(key: &str, text: &str) -> impl FnOnce(&mut Tx<'_>) -> Result<String, Error> {
    let (key, text) = (key.to_owned(), text.to_owned());
    move |tx| {
        let uid = (tx.state()?.resolve(&key))
            .map(|issue| issue.uid().to_string())
            .ok_or_else(|| Error::Validation(format!("Issue not found: {key}")))?;
        let mut data = Object::new();
        data.insert("text".into(), Value::from(text.as_str()));
        tx.emit("comment", Some(&uid), data, None)?;
        Ok(text)
    }
}

/// A write that keeps its transaction hands over what it held, and writes exactly what `writing`
/// does: the comment is appended to the journal, the store's lock is let go of, and a body that
/// fails writes nothing and hands over nothing (roll-lqjj.42).
#[test]
fn a_write_hands_over_its_transaction_where_it_is_asked_to() {
    let sandbox = sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let key = text(&a.bead(["create", "x"]).expect("an issue")["id"]);
    let ws = workspace(&a, sandbox.origin(), &[]);
    let actor = actor(&a);

    let (said, spent) =
        writing_spent(&ws, &actor, comment_on(&key, "kept")).expect("a write that keeps");
    assert_eq!(said, "kept");
    assert!(
        !spent.is_empty(),
        "the transaction's projection is handed over"
    );
    assert!(ws.wrote(), "the write is on record");
    assert_eq!(comments(&a.show(&key)), ["kept"]);

    // the lock is not held while the transaction is: another write goes through
    let said = writing(&ws, &actor, comment_on(&key, "freed")).expect("a write that frees");
    assert_eq!(said, "freed");
    assert_eq!(comments(&a.show(&key)), ["kept", "freed"]);
    drop(spent);

    // a body that fails writes nothing, whichever way the write was asked
    let failing = |tx: &mut Tx<'_>| -> Result<String, Error> {
        comment_on(&key, "lost")(tx)?;
        Err(Error::Validation("refused".to_owned()))
    };
    assert!(writing_spent(&ws, &actor, failing).is_err());
    assert!(writing(&ws, &actor, failing).is_err());
    assert_eq!(comments(&a.show(&key)), ["kept", "freed"]);
}
