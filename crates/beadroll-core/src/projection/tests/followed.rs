//! A child made under its parent's key (`P.n`) follows the parent its create names by id
//! (`parent`, which `bead create --parent` writes) when a merge renamed that parent: its key is
//! settled at its create, before anything binds to it, and every way a state is made agrees. A
//! create that names no parent, as released versions and imports wrote them, keeps its key, whatever
//! edge follows it.

use super::awaiting::{THERE, at, base, creating, every_way, on};
use super::hostile::{HERE, id};
use super::numbered::{pairs, renames};
use crate::id::EventId;
use crate::json::{Object, Value};
use crate::state::State;

use super::super::Record;

/// A create `bead create` wrote under `key`, naming no parent: not an import's.
fn written(key: &str) -> Object {
    let mut data = creating(key, vec![]);
    data.shift_remove("imported");
    data
}

/// A create `bead create --parent` wrote under `key`, made under the issue numbered `parent`.
fn under(key: &str, parent: u128) -> Object {
    let mut data = written(key);
    data.insert("parent".into(), Value::from(id(parent)));
    data
}

/// A parent-child edge to the issue numbered `n`, as `bead create --parent` writes it after the
/// create.
fn parent_of(n: u128) -> Object {
    let mut data = Object::new();
    data.insert("to".into(), Value::from(id(n)));
    data.insert("type".into(), Value::from("parent-child"));
    data
}

/// The key of the issue numbered `n` in `state`.
fn key(state: &State, n: u128) -> String {
    let uid = EventId::parse(&id(n)).unwrap();
    state.issue(uid).unwrap().key().to_owned()
}

/// A at THERE and B here, both minted `pr-base.1` under the base issue: A's sorts first, and B is
/// `pr-base.2`. Events 10, 11 (THERE) and 20, 21 (here, after `after`).
fn two_firsts(after: u128) -> Vec<Record> {
    vec![
        at(10, THERE, &[2], "create", 10, under("pr-base.1", 2)),
        at(11, THERE, &[10], "dep_add", 10, parent_of(2)),
        at(20, HERE, &[after], "create", 20, under("pr-base.1", 2)),
        at(21, HERE, &[20], "dep_add", 20, parent_of(2)),
    ]
}

/// A create naming its parent, whose key a merge renamed, takes the parent's key now with its
/// number: C, minted `pr-base.1.1` under B, is `pr-base.2.1`. A create that names no parent keeps
/// its key, whatever edge its writer adds next (E), and so does an import, whatever it carries (F).
/// Every way of making the state agrees.
#[test]
fn a_child_follows_the_parent_its_create_names() {
    let mut rest = two_firsts(2);
    rest.extend([
        at(22, HERE, &[21], "create", 22, under("pr-base.1.1", 20)),
        at(23, HERE, &[22], "dep_add", 22, parent_of(20)),
        at(30, HERE, &[23], "create", 30, written("pr-base.1.5")),
        at(31, HERE, &[30], "dep_add", 30, parent_of(20)),
    ]);
    let mut imported = creating("pr-base.1.7", vec![]);
    imported.insert("parent".into(), Value::from(id(20)));
    rest.push(at(40, HERE, &[31], "create", 40, imported));
    let (state, _) = every_way(&base(), &rest);
    let keys = [10, 20, 22, 30, 40].map(|n| key(&state, n));
    let expected = [
        "pr-base.1",
        "pr-base.2",
        "pr-base.2.1",
        "pr-base.1.5",
        "pr-base.1.7",
    ];
    assert_eq!(keys, expected);
    let renamed = [("pr-base.1", "pr-base.2"), ("pr-base.1.1", "pr-base.2.1")];
    assert_eq!(renames(&state), pairs(&renamed));
}

/// A child's key is settled at its create, so nothing binds to the key it was minted under and
/// leaves: an import waiting for `pr-base.1.1` binds to A's child E, which holds it, not to C,
/// minted under it and moved to `pr-base.2.1`; E, made between C's create and C's edge, keeps
/// `pr-base.1.1`.
#[test]
fn a_childs_key_is_settled_at_its_create() {
    let waiting = creating("pr-imp", vec![on("pr-base.1.1", None, &[])]);
    let mut rest = vec![at(15, HERE, &[2], "create", 15, waiting)];
    rest.extend(two_firsts(15));
    rest.extend([
        at(22, HERE, &[21], "create", 22, under("pr-base.1.1", 20)),
        at(23, THERE, &[11], "create", 23, under("pr-base.1.1", 10)),
        at(24, HERE, &[22], "dep_add", 22, parent_of(20)),
        at(26, THERE, &[23], "dep_add", 23, parent_of(10)),
    ]);
    let (state, _) = every_way(&base(), &rest);
    assert!(state.integrity().is_empty(), "{:?}", state.integrity());
    let keys = [22, 23].map(|n| key(&state, n));
    assert_eq!(keys, ["pr-base.2.1", "pr-base.1.1"]);
    let imported = state.issue(EventId::parse(&id(15)).unwrap()).unwrap();
    assert!(imported.dangling().is_empty(), "{:?}", imported.dangling());
    let bound: Vec<String> = (imported.deps().iter())
        .map(|(edge, _)| state.target_key(edge.to()).into_owned())
        .collect();
    assert_eq!(bound, ["pr-base.1.1"]);
}

/// A parent-child edge its writer adds next to a create that names no parent moves nothing, on one
/// machine and with no merge: a create whose own edge a torn append lost, then `dep add` of the
/// same child under another parent. It took that parent's key, and the key `dep add` printed no
/// longer resolved.
#[test]
fn an_edge_after_a_create_moves_no_key() {
    let rest = [
        at(10, HERE, &[2], "create", 10, written("pr-p")),
        at(11, HERE, &[10], "create", 11, written("pr-base.1")),
        at(12, HERE, &[11], "dep_add", 11, parent_of(10)),
    ];
    let (state, _) = every_way(&base(), &rest);
    assert_eq!(key(&state, 11), "pr-base.1");
    assert!(state.renames().is_empty(), "{:?}", state.renames());
}

/// A create whose `parent` is not an event id is reported, as every malformed member is, and makes
/// no issue.
#[test]
fn a_malformed_parent_is_reported() {
    let mut data = written("pr-base.1");
    data.insert("parent".into(), Value::from(5));
    let (state, _) = every_way(&base(), &[at(10, HERE, &[2], "create", 10, data)]);
    assert!(state.issue(EventId::parse(&id(10)).unwrap()).is_none());
    let problems: Vec<&str> = (state.integrity().iter())
        .map(|entry| entry.problem.as_str())
        .collect();
    assert!(
        problems.iter().any(|p| p.contains("'parent'")),
        "{problems:?}"
    );
}

/// A key a child follows to is passed over when an import waits for it, as an issue's would be:
/// C, minted `pr-base.1.1` under B, follows B to `pr-base.2`, and the import D waits for
/// `pr-base.2.1`, so C is `pr-base.2.2` and D keeps waiting. C took `pr-base.2.1`, and D bound to
/// it.
#[test]
fn a_followed_key_passes_over_one_an_import_waits_for() {
    let waiting = creating("pr-imp", vec![on("pr-base.2.1", None, &[])]);
    let mut rest = vec![at(15, HERE, &[2], "create", 15, waiting)];
    rest.extend(two_firsts(15));
    rest.push(at(22, HERE, &[21], "create", 22, under("pr-base.1.1", 20)));
    let (state, _) = every_way(&base(), &rest);
    assert_eq!(key(&state, 22), "pr-base.2.2");
    let imported = state.issue(EventId::parse(&id(15)).unwrap()).unwrap();
    assert_eq!(
        imported.dangling().len(),
        1,
        "the import bound to the child"
    );
}

/// A key with no parent part before its number (`.1`, which no issue's key could have been minted
/// under) keeps its spelling, whatever parent its create names; it took its parent's key.
#[test]
fn a_key_with_no_parent_part_keeps_its_spelling() {
    let rest = [at(10, HERE, &[2], "create", 10, under(".1", 2))];
    let (state, _) = every_way(&base(), &rest);
    assert_eq!(key(&state, 10), ".1");
}

/// A child under a parent that gave way settles alike made every way, where the parent's stem has a
/// newline (its copy takes another stem) and the child first meets the other machine's child of the
/// same number: C, minted under P2's key, follows P2.
#[test]
fn a_child_of_a_parent_with_a_newline_settles_every_way() {
    let rest = [
        at(10, THERE, &[2], "create", 10, written("pr-a\nb")),
        at(11, THERE, &[10], "create", 11, under("pr-a\nb.1", 10)),
        at(20, HERE, &[2], "create", 20, written("pr-a\nb")),
        at(21, HERE, &[20], "create", 21, under("pr-a\nb.1", 20)),
    ];
    let (state, _) = every_way(&base(), &rest);
    assert_eq!(key(&state, 21), format!("{}.1", key(&state, 20)));
}
