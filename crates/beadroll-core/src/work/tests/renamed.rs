//! A copy of a key two machines minted alike (roll-lqjj.25, X03): replay finds the copy's key by
//! asking of each candidate whether an issue or an import has it, and never by collecting every
//! key the tracker claims, so renaming N copies of plain keys (`ex-1` to `ex-N`) asks a bounded
//! number of keys for each. A numbered key (`ex-1.1`) is counted upward past every number taken
//! (`key::free_key`), so N copies of those ask N² keys; that is not guarded here (roll-lqjj.46).

use crate::journal::Journal;
use crate::json::{Object, Value};
use crate::replay::replay;

use super::super::Work;
use super::{MACHINE, grows_linearly, id, key};

/// The machine whose creates sort second, and are the copies.
const OTHER: &str = "b";

/// Event `n` of `machine`, built on `after`: the `init` of the first machine's, or the create of
/// an imported issue under `key`, importing a dependency on `awaited` when given.
fn event(
    n: u64,
    machine: &str,
    after: Option<u64>,
    key: Option<&str>,
    awaited: Option<String>,
) -> Value {
    let mut data = Object::new();
    match key {
        None => data.insert("prefix".into(), Value::from("ex")),
        Some(key) => {
            let mut fields = Object::new();
            fields.insert("title".into(), Value::from(key));
            data.insert("key".into(), Value::from(key));
            data.insert("fields".into(), Value::from(fields));
            data.insert("imported".into(), Value::from(true))
        }
    };
    if let Some(awaited) = awaited {
        let mut dep = Object::new();
        dep.insert("depends_on_id".into(), Value::from(awaited));
        dep.insert("type".into(), Value::from("blocks"));
        data.insert("dangling".into(), Value::Array(vec![Value::from(dep)]));
    }
    let mut event = Object::new();
    event.insert("v".into(), Value::from(1));
    event.insert("id".into(), Value::from(id(n)));
    event.insert("replica".into(), Value::from(machine));
    let deps = after.map(|after| Value::from(id(after)));
    event.insert("deps".into(), Value::Array(deps.into_iter().collect()));
    event.insert("op".into(), Value::from(key.map_or("init", |_| "create")));
    event.insert("issue".into(), Value::from(id(n)));
    event.insert("data".into(), Value::from(data));
    Value::from(event)
}

/// The journal of two machines that each create `ex-1` to `ex-N`, as two machines importing one
/// export before they sync do, after one `init`: each machine's creates in a chain of its own, and
/// those of the second after every one of the first's, so that its N creates are the copies and
/// are renamed. When `waiting`, every issue of the first also imports a dependency on a key that
/// never comes, so that the keys claimed are the issues' and the keys waited for together.
fn minted_twice(issues: usize, waiting: bool) -> Journal {
    let mut journal = Journal::new();
    journal.add(event(1, MACHINE, None, None, None), MACHINE, "", true);
    for (turn, machine) in [MACHINE, OTHER].into_iter().enumerate() {
        for n in 1..=issues {
            let number = (turn * issues + n) as u64 + 1;
            let after = if n == 1 { 1 } else { number - 1 };
            let awaited = (waiting && turn == 0).then(|| format!("gone-{n}"));
            let created = event(number, machine, Some(after), Some(&key(n)), awaited);
            journal.add(created, machine, "", true);
        }
    }
    journal
}

#[test]
fn renaming_every_copy_of_keys_two_machines_minted_reads_a_bounded_number_of_keys() {
    for waiting in [false, true] {
        grows_linearly(
            &format!("replaying two machines' creates of the same keys, waiting: {waiting}"),
            &[Work::Key, Work::Waiting],
            |issues| (minted_twice(issues, waiting), issues),
            |(journal, issues)| {
                let state = replay(journal).unwrap();
                // every copy of the second machine was kept, renamed, and what is asserted here
                // reads no key through a counted accessor
                assert_eq!(state.issues().count(), 2 * issues);
                assert_eq!(state.renames().len(), issues);
            },
        );
    }
}
