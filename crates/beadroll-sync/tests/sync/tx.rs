//! A transaction's state while it emits ([`Tx::lend`], roll-lqjj.29): what is lent is the state
//! before, and the next state asked for has every event emitted since, from the projection or
//! from the journal replayed; and what a write does with its transaction once it is done
//! ([`writing`] lets go of it, [`writing_spent`] hands it over, roll-lqjj.42).

use beadroll_core::id::EventId;
use beadroll_core::json::{Object, Value};
use beadroll_core::op::OpName;
use beadroll_sync::{
    Error, NetworkMode, Tx, locked, publish_soon, writing, writing_spent, writing_state,
};
use beadroll_test_support::Machine;

use crate::support::{actor, comments, sandbox, text, workspace};

/// Preparation preserves both destinations, including events emitted after the last cached
/// state read. The returned state and the persisted journal must contain the same whole write.
/// Every record of the journal comes before the attempt's first event: one after it is refused.
#[test]
fn preparing_a_write_keeps_sequential_and_conditional_events_together() {
    let sandbox = sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let key = text(&a.bead(["create", "x"]).expect("an issue")["id"]);
    let ws = workspace(&a, sandbox.origin(), &[]);
    let actor = actor(&a);
    let (_, state) = writing_state(&ws, &actor, |tx| -> Result<(), Error> {
        comment_on(&key, "before the read")(tx)?;
        let uid = tx.state()?.resolve(&key).expect("the issue").uid();
        comment_on(&key, "before the claim")(tx)?;
        crate::support::claim(tx, beadroll_sync::Mode::Local, &key)?;
        tx.attempt().emit(
            OpName::Set,
            Some(&uid.to_string()),
            titled("with the claim"),
            None,
        )?;
        let after = comment_on(&key, "after the attempt")(tx);
        assert!(matches!(after, Err(Error::Validation(_))), "{after:?}");
        Ok(())
    })
    .expect("the mixed write");
    let state = state.expect("the prepared final state");
    let issue = state.resolve(&key).expect("the issue");
    assert_eq!(issue.title(), "with the claim");
    assert!(issue.claim().is_some());
    let shown = a.show(&key);
    assert_eq!(shown["title"].as_str(), Some("with the claim"));
    assert_eq!(comments(&shown), ["before the read", "before the claim"]);
    assert!(shown["claim"].as_object().is_some());
}

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
                tx.emit(OpName::Set, Some(&id), titled(name), None)?;
                assert_eq!(title(&mut tx, uid), name);
            }
            {
                let (st, mut emitter) = tx.lend()?;
                emitter.emit(OpName::Set, Some(&id), titled("three"), None)?;
                emitter.emit(OpName::Set, Some(&id), titled("four"), None)?;
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
        tx.emit(OpName::Comment, Some(&uid), data, None)?;
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

/// Returning the state happens after the write drains the emitted records. Both incremental
/// application and first-record replay must retain the final view through that handoff.
#[test]
fn a_written_state_survives_draining_and_publication_preserves_commitment() {
    let sandbox = sandbox();
    let (a, b) = (Machine::new(&sandbox, "a"), Machine::new(&sandbox, "b"));
    a.bead(["init", "--prefix", "ex"]);
    let key = text(&a.bead(["create", "x"]).expect("an issue")["id"]);
    for machine in [&a, &b] {
        machine.bead(["sync"]);
        let ws = workspace(machine, sandbox.origin(), &[]);
        let actor = actor(machine);
        assert!(!ws.committed());
        assert!(!ws.wrote());
        let failed = writing(&ws, &actor, |tx| -> Result<(), Error> {
            comment_on(&key, "discarded")(tx)?;
            Err(Error::Validation("refused".to_owned()))
        });
        assert!(failed.is_err());
        assert!(!ws.committed(), "a failed body has no durable effect");
        assert!(!ws.wrote());

        let (said, state) = writing_state(&ws, &actor, |tx| -> Result<String, Error> {
            let uid = tx.state()?.resolve(&key).expect("the issue").uid();
            tx.emit(
                OpName::Set,
                Some(&uid.to_string()),
                titled(machine.name()),
                None,
            )?;
            Ok(machine.name().to_owned())
        })
        .expect("a write with its state");
        let state = state.expect("the final view survives draining");
        assert_eq!(state.resolve(&key).expect("the issue").title(), said);
        assert!(ws.committed());
        assert!(ws.wrote());
        assert_eq!(publish_soon(&ws, NetworkMode::Online, None), None);
        assert!(!ws.wrote(), "publication clears the scheduled work");
        assert!(ws.committed(), "publication cannot undo the durable effect");
        assert_eq!(machine.show(&key)["title"].as_str(), Some(said.as_str()));
        assert!(comments(&machine.show(&key)).is_empty());
    }
}

/// A create's parent is named among that create's causes alone: a child refused (a priority of the
/// wrong type), the comment after it builds on the heads as they were, where it named the
/// refused create's parent too (AT100).
#[test]
fn a_refused_create_leaves_its_parent_out_of_the_next_events_causes() {
    let sandbox = sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let parent = text(&a.bead(["create", "p"]).expect("an issue")["id"]);
    let other = text(&a.bead(["create", "q"]).expect("an issue")["id"]);
    let ws = workspace(&a, sandbox.origin(), &[]);
    let actor = actor(&a);
    let (parent_id, comment) = writing(&ws, &actor, |tx| -> Result<(EventId, EventId), Error> {
        let uid = tx.state()?.resolve(&parent).expect("the issue").uid();
        let commented = tx.state()?.resolve(&other).expect("the issue").uid();
        let mut fields = Object::new();
        fields.insert("title".into(), Value::from("child"));
        fields.insert("priority".into(), Value::from(true));
        let mut data = Object::new();
        data.insert("key".into(), Value::from(format!("{parent}.1")));
        data.insert("fields".into(), Value::from(fields));
        data.insert("labels".into(), Value::Array(Vec::new()));
        data.insert("parent".into(), Value::from(uid.to_string()));
        let refused = tx.emit(OpName::Create, None, data, None);
        assert!(matches!(refused, Err(Error::Validation(_))), "{refused:?}");
        let mut said = Object::new();
        said.insert("text".into(), Value::from("after the refusal"));
        let comment = tx.emit(OpName::Comment, Some(&commented.to_string()), said, None)?;
        Ok((uid, comment))
    })
    .expect("a write");
    let mut events = Vec::new();
    for segment in beadroll_store::segment_files(&ws).expect("the segments") {
        let text = std::fs::read_to_string(segment).expect("a segment");
        events.extend(
            text.lines()
                .map(|line| beadroll_test_support::parse_json(line).unwrap()),
        );
    }
    let written = (events.iter())
        .find(|event| event["id"] == beadroll_test_support::Json::from(comment.to_string()))
        .expect("the comment");
    let deps = written["deps"].as_array().expect("its causes");
    let named = beadroll_test_support::Json::from(parent_id.to_string());
    assert!(!deps.contains(&named), "{deps:?}");
}

/// A create's parent must be an issue the transaction holds, whichever way it is emitted: a
/// comment's event, though seen, is refused as a parent by `Tx::emit` and by a lent emitter, where
/// it was written and replay found no parent to follow, keeping the minted key (AT100).
#[test]
fn a_create_whose_parent_is_no_issue_is_refused() {
    let sandbox = sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let key = text(&a.bead(["create", "x"]).expect("an issue")["id"]);
    let ws = workspace(&a, sandbox.origin(), &[]);
    let actor = actor(&a);
    let create = |parent: &str| {
        let mut fields = Object::new();
        fields.insert("title".into(), Value::from("child"));
        let mut data = Object::new();
        data.insert("key".into(), Value::from(format!("{key}.1")));
        data.insert("fields".into(), Value::from(fields));
        data.insert("labels".into(), Value::Array(Vec::new()));
        data.insert("parent".into(), Value::from(parent));
        data
    };
    locked(&ws, || -> Result<(), Error> {
        let mut tx = Tx::new(&ws, &actor)?;
        let uid = tx.state()?.resolve(&key).expect("the issue").uid();
        let mut said = Object::new();
        said.insert("text".into(), Value::from("a comment"));
        let comment = tx.emit(OpName::Comment, Some(&uid.to_string()), said, None)?;
        let refused = tx.emit(OpName::Create, None, create(&comment.to_string()), None);
        assert!(matches!(refused, Err(Error::Validation(_))), "{refused:?}");
        {
            let (_, mut emitter) = tx.lend()?;
            let lent = emitter.emit(OpName::Create, None, create(&comment.to_string()), None);
            assert!(matches!(lent, Err(Error::Validation(_))), "{lent:?}");
        }
        tx.emit(OpName::Create, None, create(&uid.to_string()), None)?;
        Ok(())
    })
    .expect("a transaction");
}

/// A create under a parent is judged for its structure, as every path emits it: a key with no
/// number after a parent part refused, where replay had no key to give its number; a child of a
/// parent the same lent emitter made just before, and siblings of one parent, taken. Its key is
/// the caller's to mint from the parent's (AT100).
#[test]
fn a_create_under_a_parent_is_judged_for_its_structure() {
    let sandbox = sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let key = text(&a.bead(["create", "x"]).expect("an issue")["id"]);
    let ws = workspace(&a, sandbox.origin(), &[]);
    let actor = actor(&a);
    let create = |key: &str, parent: Option<&str>| {
        let mut fields = Object::new();
        fields.insert("title".into(), Value::from(key));
        let mut data = Object::new();
        data.insert("key".into(), Value::from(key));
        data.insert("fields".into(), Value::from(fields));
        data.insert("labels".into(), Value::Array(Vec::new()));
        if let Some(parent) = parent {
            data.insert("parent".into(), Value::from(parent));
        }
        data
    };
    locked(&ws, || -> Result<(), Error> {
        let mut tx = Tx::new(&ws, &actor)?;
        let uid = tx
            .state()?
            .resolve(&key)
            .expect("the issue")
            .uid()
            .to_string();
        let unnumbered = tx.emit(
            OpName::Create,
            None,
            create(&format!("{key}.x"), Some(&uid)),
            None,
        );
        assert!(
            matches!(unnumbered, Err(Error::Validation(_))),
            "{unnumbered:?}"
        );
        let (_, mut emitter) = tx.lend()?;
        let parent = emitter.emit(OpName::Create, None, create("ex-made", None), None)?;
        let child = create("ex-made.1", Some(&parent.to_string()));
        emitter.emit(OpName::Create, None, child, None)?;
        for n in [7, 8] {
            let sibling = create(&format!("{key}.{n}"), Some(&uid));
            emitter.emit(OpName::Create, None, sibling, None)?;
        }
        Ok(())
    })
    .expect("a transaction");
}

/// The data of a create of `key`, under `parent` where one is given.
fn create_of(key: &str, parent: Option<EventId>) -> Object {
    let mut fields = Object::new();
    fields.insert("title".into(), Value::from(key));
    let mut data = Object::new();
    data.insert("key".into(), Value::from(key));
    data.insert("fields".into(), Value::from(fields));
    data.insert("labels".into(), Value::Array(Vec::new()));
    if let Some(parent) = parent {
        data.insert("parent".into(), Value::from(parent.to_string()));
    }
    data
}

/// A create built on a cause that waits for one of its own makes no issue until it comes, and is
/// no parent of a create after it in the transaction whether or not the state was read between
/// them, where the transaction took it for one until a read applied it (AT102).
#[test]
fn a_create_built_on_a_waiting_cause_is_no_parent_however_the_transaction_is_read() {
    let sandbox = sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let ws = workspace(&a, sandbox.origin(), &[]);
    // a foreign event waiting for a cause no journal holds: a head no write applies beside
    let waiting = beadroll_test_support::events::id_at(1_790_000_000_001, 1);
    let missing = beadroll_test_support::events::id_at(1_790_000_000_002, 2);
    let event = beadroll_test_support::json!({
        "v": 1, "id": (waiting.as_str()), "replica": "0000000f", "deps": [(missing.as_str())],
        "actor": {"agent": "test", "thread": "f"},
        "op": "comment", "issue": (missing.as_str()), "data": {"text": "waits"}
    });
    beadroll_store::append_events(&ws, "0000000f", [&event]).expect("the foreign journal");
    let actor = actor(&a);
    for read_between in [false, true] {
        locked(&ws, || -> Result<(), Error> {
            let mut tx = Tx::new(&ws, &actor)?;
            let parent = tx.emit(OpName::Create, None, create_of("ex-p", None), None)?;
            if read_between {
                let state = tx.state()?;
                assert!(state.issue(parent).is_none(), "the parent waits");
            }
            let child = create_of("ex-p.1", Some(parent));
            let judged = tx.emit(OpName::Create, None, child, None);
            assert!(
                matches!(judged, Err(Error::Validation(_))),
                "read between: {read_between}: {judged:?}"
            );
            Ok(())
        })
        .expect("a transaction");
    }
}

/// A transaction on a journal whose chain is broken already, every record of it set aside by
/// replay, takes none of the creates it records there for a parent, and takes those of an attempt,
/// in no journal's chain and applied, for a parent of the attempt's own creates (not of the
/// journal's, appended before the attempt is published), whether or not the state was read
/// between, as the state says once read; `writing` refuses such a journal before its body runs
/// (AT102).
#[test]
fn a_create_on_a_broken_chain_is_no_parent_however_the_transaction_is_read() {
    let sandbox = sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    a.bead(["create", "x"]);
    let replica = a.id();
    let ws = workspace(&a, sandbox.origin(), &[]);
    // a record of this machine's built on nothing: it does not build on the one before it
    let stray = beadroll_test_support::events::id_at(1_790_000_000_001, 1);
    let event = beadroll_test_support::json!({
        "v": 1, "id": (stray.as_str()), "replica": (replica.as_str()), "deps": [],
        "actor": {"agent": "test", "thread": "a"},
        "op": "comment", "issue": (stray.as_str()), "data": {"text": "stray"}
    });
    beadroll_store::append_events(&ws, &replica, [&event]).expect("the stray record");
    let actor = actor(&a);
    for read_between in [false, true] {
        locked(&ws, || -> Result<(), Error> {
            let mut tx = Tx::new(&ws, &actor)?;
            assert!(tx.state()?.chain_breaks().contains_key(&replica), "broken");
            let parent = tx.emit(OpName::Create, None, create_of("ex-p", None), None)?;
            if read_between {
                assert!(tx.state()?.issue(parent).is_none(), "set aside");
            }
            let child = create_of("ex-p.1", Some(parent));
            let judged = tx.emit(OpName::Create, None, child, None);
            assert!(
                matches!(judged, Err(Error::Validation(_))),
                "read between: {read_between}: {judged:?}"
            );
            Ok(())
        })
        .expect("a transaction");
        // an attempt's create is in no journal's chain, and applies: its child is taken
        locked(&ws, || -> Result<(), Error> {
            let mut tx = Tx::new(&ws, &actor)?;
            let parent = tx
                .attempt()
                .emit(OpName::Create, None, create_of("ex-q", None), None)?;
            if read_between {
                assert!(tx.state()?.issue(parent).is_some(), "applied");
            }
            let child = create_of("ex-q.1", Some(parent));
            let judged = tx.attempt().emit(OpName::Create, None, child, None);
            assert!(judged.is_ok(), "read between: {read_between}: {judged:?}");
            Ok(())
        })
        .expect("an attempt");
    }
}

/// A placeholder that broke this machine's journal chain, malformed and built on nothing, is no
/// predecessor: a create after it builds on the valid record before it and applies, and is a
/// parent whether or not the state was read between, where every break was taken for one that
/// sets aside each record after it (AT102).
#[test]
fn a_create_after_a_placeholder_that_broke_the_chain_is_a_parent() {
    let sandbox = sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    a.bead(["create", "x"]);
    let replica = a.id();
    let ws = workspace(&a, sandbox.origin(), &[]);
    let stray = beadroll_test_support::events::id_at(1_790_000_000_001, 1);
    let event = beadroll_test_support::json!({
        "v": 1, "id": (stray.as_str()), "replica": (replica.as_str()), "deps": [],
        "actor": {"agent": "test", "thread": "a"},
        "op": 7, "issue": (stray.as_str()), "data": {}
    });
    beadroll_store::append_events(&ws, &replica, [&event]).expect("the placeholder");
    let actor = actor(&a);
    for read_between in [false, true] {
        locked(&ws, || -> Result<(), Error> {
            let mut tx = Tx::new(&ws, &actor)?;
            assert!(tx.state()?.chain_breaks().contains_key(&replica), "a break");
            let parent = tx.emit(OpName::Create, None, create_of("ex-p", None), None)?;
            if read_between {
                assert!(tx.state()?.issue(parent).is_some(), "applied");
            }
            let child = create_of("ex-p.1", Some(parent));
            let judged = tx.emit(OpName::Create, None, child, None);
            assert!(judged.is_ok(), "read between: {read_between}: {judged:?}");
            Ok(())
        })
        .expect("a transaction");
    }
}
