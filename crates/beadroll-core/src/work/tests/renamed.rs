//! A copy of a key two machines minted alike (roll-lqjj.25, X03): replay finds the copy's key by
//! asking of each candidate whether an issue or an import has it, and never by collecting every
//! key the tracker claims, so renaming N copies of plain keys (`ex-1` to `ex-N`) asks a bounded
//! number of keys for each. A numbered key (`ex-1.1`) is counted upward past every number taken
//! (`key::free_key`), and N copies of those once asked about N² keys: a copy now skips the runs of
//! numbers taken that copies before it found (`state::Runs`, roll-lqjj.46), so the first copy asks
//! of every number it counts past and each after it of a bounded number, whether the numbers are
//! taken by issues or waited for, and however many keys taken back open gaps in the runs.

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

/// The keys two machines both mint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Keys {
    /// `ex-1` to `ex-N`, each copy given its digest's shortest free prefix.
    Plain,
    /// The children `ex-1.1` to `ex-1.N`, each copy counting upward past every number taken.
    Numbered,
}

/// The journal of two machines that each create the N keys of `keys`, as two machines importing
/// one export before they sync do, after one `init`: each machine's creates in a chain of its own,
/// and those of the second after every one of the first's, so that its N creates are the copies and
/// are renamed. When `waiting`, every issue of the first also imports a dependency on a key that
/// never comes, so that the keys claimed are the issues' and the keys waited for together: of
/// numbered keys, the next N numbers, `ex-1.N+1` to `ex-1.2N`, which the copies count past too.
fn minted_twice(issues: usize, keys: Keys, waiting: bool) -> Journal {
    let named = |n: usize| match keys {
        Keys::Plain => key(n),
        Keys::Numbered => format!("ex-1.{n}"),
    };
    let mut journal = Journal::new();
    journal.add(event(1, MACHINE, None, None, None), MACHINE, "", true);
    for (turn, machine) in [MACHINE, OTHER].into_iter().enumerate() {
        for n in 1..=issues {
            let number = (turn * issues + n) as u64 + 1;
            let after = if n == 1 { 1 } else { number - 1 };
            let awaited = (waiting && turn == 0).then(|| match keys {
                Keys::Plain => format!("gone-{n}"),
                Keys::Numbered => named(issues + n),
            });
            let created = event(number, machine, Some(after), Some(&named(n)), awaited);
            journal.add(created, machine, "", true);
        }
    }
    journal
}

#[test]
fn renaming_every_copy_of_keys_two_machines_minted_reads_a_bounded_number_of_keys() {
    for keys in [Keys::Plain, Keys::Numbered] {
        for waiting in [false, true] {
            grows_linearly(
                &format!(
                    "replaying two machines' creates of the same keys, {keys:?}, waiting: \
                     {waiting}"
                ),
                &[Work::Key, Work::Waiting],
                |issues| (minted_twice(issues, keys, waiting), issues),
                |(journal, issues)| {
                    let state = replay(journal).unwrap();
                    // every copy of the second machine was kept, renamed, and what is asserted
                    // here reads no key through a counted accessor
                    assert_eq!(state.issues().count(), 2 * issues);
                    assert_eq!(state.renames().len(), issues);
                    if keys == Keys::Numbered {
                        // copy n past the first machine's numbers, and the numbers waited for
                        let past = if waiting { 2 * issues } else { issues };
                        for (n, rename) in (1..).zip(state.renames()) {
                            assert_eq!(rename.to, format!("ex-1.{}", past + n));
                        }
                    }
                },
            );
        }
    }
}

/// Event `n` of `machine`, built on `deps`, doing `op` on the issue event `issue` created.
fn doing(n: u64, machine: &str, deps: &[u64], op: &str, issue: u64, data: Object) -> Value {
    let mut event = Object::new();
    event.insert("v".into(), Value::from(1));
    event.insert("id".into(), Value::from(id(n)));
    event.insert("replica".into(), Value::from(machine));
    let deps = deps.iter().map(|dep| Value::from(id(*dep))).collect();
    event.insert("deps".into(), Value::Array(deps));
    event.insert("op".into(), Value::from(op));
    event.insert("issue".into(), Value::from(id(issue)));
    event.insert("data".into(), Value::from(data));
    Value::from(event)
}

/// The journal of a first machine that creates `ex-1.1` to `ex-1.N` and then an issue whose import
/// waits for `ex-1.N+1` to `ex-1.2N`, and of a second whose copy of `ex-1.1` counts past all of
/// them, to `ex-1.2N+1`; then, N times, the first takes back what the import waits for of the next
/// of those keys, and the second, having seen it, makes another copy of `ex-1.1`, which takes it. A
/// copy finds the key it takes inside the run of numbers copies found claimed, which only taking
/// back the one key out of the run keeps a step away.
fn taken_back_one_at_a_time(issues: usize) -> Journal {
    let mut journal = Journal::new();
    journal.add(event(1, MACHINE, None, None, None), MACHINE, "", true);
    let mut n = 1;
    for number in 1..=issues {
        n += 1;
        journal.add(
            event(
                n,
                MACHINE,
                Some(n - 1),
                Some(&format!("ex-1.{number}")),
                None,
            ),
            MACHINE,
            "",
            true,
        );
    }
    let named = (issues + 1..=2 * issues).map(|number| {
        let mut dep = Object::new();
        dep.insert(
            "depends_on_id".into(),
            Value::from(format!("ex-1.{number}")),
        );
        Value::from(dep)
    });
    let mut data = Object::new();
    data.insert("key".into(), Value::from("ex-y"));
    data.insert("imported".into(), Value::from(true));
    data.insert("dangling".into(), Value::Array(named.collect()));
    n += 1;
    let (waits, mut first) = (n, n);
    journal.add(
        doing(n, MACHINE, &[n - 1], "create", n, data),
        MACHINE,
        "",
        true,
    );
    let copy = |n: u64, deps: &[u64]| {
        let mut data = Object::new();
        data.insert("key".into(), Value::from("ex-1.1"));
        data.insert("imported".into(), Value::from(true));
        doing(n, OTHER, deps, "create", n, data)
    };
    n += 1;
    let mut second = n;
    journal.add(copy(n, &[1]), OTHER, "", true);
    for number in issues + 1..=2 * issues {
        n += 1;
        let mut data = Object::new();
        data.insert("key".into(), Value::from(format!("ex-1.{number}")));
        data.insert("type".into(), Value::Null);
        journal.add(
            doing(n, MACHINE, &[first], "dep_remove", waits, data),
            MACHINE,
            "",
            true,
        );
        first = n;
        n += 1;
        journal.add(copy(n, &[second, first]), OTHER, "", true);
        second = n;
    }
    journal
}

#[test]
fn a_copy_of_a_numbered_key_taken_back_inside_a_run_reads_a_bounded_number_of_keys() {
    grows_linearly(
        "replaying a copy after each key an import waited for is taken back",
        &[Work::Key, Work::Waiting],
        |issues| (taken_back_one_at_a_time(issues), issues),
        |(journal, issues)| {
            let state = replay(journal).unwrap();
            assert_eq!(state.renames().len(), issues + 1);
            let (first, rest) = state.renames().split_first().unwrap();
            assert_eq!(first.to, format!("ex-1.{}", 2 * issues + 1));
            for (n, rename) in (1..).zip(rest) {
                assert_eq!(rename.to, format!("ex-1.{}", issues + n));
            }
        },
    );
}
