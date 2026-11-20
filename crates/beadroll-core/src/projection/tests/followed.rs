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
        at(20, HERE, &[after, 2], "create", 20, under("pr-base.1", 2)),
        at(21, HERE, &[20], "dep_add", 20, parent_of(2)),
    ]
}

/// A create naming its parent, whose key a merge renamed, takes the parent's key now with its
/// number: C, minted `pr-base.1.1` under B, is `pr-base.2.1`. A create that names no parent keeps
/// its key, whatever edge its writer adds next (E), and so does an import, whatever it carries (F).
/// Every way of making the state agrees. (AT100)
#[test]
fn a_child_follows_the_parent_its_create_names() {
    let mut rest = two_firsts(2);
    rest.extend([
        at(22, HERE, &[21, 20], "create", 22, under("pr-base.1.1", 20)),
        at(23, HERE, &[22], "dep_add", 22, parent_of(20)),
        at(30, HERE, &[23], "create", 30, written("pr-base.1.5")),
        at(31, HERE, &[30], "dep_add", 30, parent_of(20)),
    ]);
    let mut imported = creating("pr-base.1.7", vec![]);
    imported.insert("parent".into(), Value::from(id(20)));
    // named among its causes too, so that only its being an import keeps it from following
    rest.push(at(40, HERE, &[31, 20], "create", 40, imported));
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
/// `pr-base.1.1`. (AT100)
#[test]
fn a_childs_key_is_settled_at_its_create() {
    let waiting = creating("pr-imp", vec![on("pr-base.1.1", None, &[])]);
    let mut rest = vec![at(15, HERE, &[2], "create", 15, waiting)];
    rest.extend(two_firsts(15));
    rest.extend([
        at(22, HERE, &[21, 20], "create", 22, under("pr-base.1.1", 20)),
        at(23, THERE, &[11, 10], "create", 23, under("pr-base.1.1", 10)),
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
/// longer resolved. (AT100)
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

/// A create whose `parent` is no event id is read as naming no parent, as replay applied such a
/// create before the member was added (`parent: null`, a key): its issue is made, under its key. A
/// write of one is refused (`event::write_problem`). (AT100)
#[test]
fn a_parent_that_is_no_id_names_no_parent() {
    let mut rest = two_firsts(2);
    for (n, parent) in [
        (22, Value::Null),
        (23, Value::from("pr-base.2")),
        (24, Value::from(5)),
    ] {
        let mut data = written("pr-base.1.1");
        data.insert("parent".into(), parent);
        rest.push(at(n, HERE, &[n - 1], "create", n, data));
    }
    let (state, _) = every_way(&base(), &rest);
    assert!(state.integrity().is_empty(), "{:?}", state.integrity());
    assert_eq!(
        key(&state, 22),
        "pr-base.1.1",
        "it followed a parent it does not name"
    );
    let event = super::hostile::event(25, HERE, &[24, 20], "create", 25, under("pr-base.1.1", 20));
    assert_eq!(crate::event::write_problem(&event), None);
    // a parent that is not among the create's causes is refused too
    let event = super::hostile::event(25, HERE, &[24], "create", 25, under("pr-base.1.1", 20));
    assert!(crate::event::write_problem(&event).is_some_and(|p| p.contains("deps")));
    // and one naming itself as its parent, its id an issue's, which would set both aside
    let event = super::hostile::event(25, HERE, &[24, 25], "create", 25, under("pr-base.1.1", 25));
    assert!(crate::event::write_problem(&event).is_some_and(|p| p.contains("itself")));
    // and one keyed with no number after a parent part
    let event = super::hostile::event(25, HERE, &[24, 20], "create", 25, under("root", 20));
    assert!(crate::event::write_problem(&event).is_some_and(|p| p.contains("PARENT.N")));
    let mut data = written("pr-base.1.1");
    data.insert("parent".into(), Value::Null);
    let event = super::hostile::event(25, HERE, &[24], "create", 25, data);
    assert!(crate::event::write_problem(&event).is_some_and(|p| p.contains("'parent'")));
}

/// A key a child follows to is passed over when an import waits for it, as an issue's would be:
/// C, minted `pr-base.1.1` under B, follows B to `pr-base.2`, and the import D waits for
/// `pr-base.2.1`, so C is `pr-base.2.2` and D keeps waiting. C took `pr-base.2.1`, and D bound to
/// it. (AT100)
#[test]
fn a_followed_key_passes_over_one_an_import_waits_for() {
    let waiting = creating("pr-imp", vec![on("pr-base.2.1", None, &[])]);
    let mut rest = vec![at(15, HERE, &[2], "create", 15, waiting)];
    rest.extend(two_firsts(15));
    rest.push(at(
        22,
        HERE,
        &[21, 20],
        "create",
        22,
        under("pr-base.1.1", 20),
    ));
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
/// under) keeps its spelling, whatever parent its create names; it took its parent's key. (AT100)
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

/// A chain of records each naming the one before as its parent, every one minted short, makes no
/// key longer than a bounded multiple of its own: each follows while its parent's key is within
/// that bound, and keeps its own once it is not, where every key had been longer than the last.
/// (AT100)
#[test]
fn a_chain_of_records_stops_growing_the_keys_it_makes() {
    let mut rest = vec![at(10, HERE, &[2], "create", 10, written("pr-x"))];
    for n in 11..111u128 {
        rest.push(at(n, HERE, &[n - 1], "create", n, under("pr-b.1", n - 1)));
    }
    let (state, _) = every_way(&base(), &rest);
    let longest = (11..111u128).map(|n| key(&state, n).len()).max().unwrap();
    assert!(
        longest <= "pr-b".len() + 64 + ".1".len(),
        "a key of {longest} bytes"
    );
    assert_eq!(
        key(&state, 11),
        "pr-x.1",
        "a parent within the bound is followed"
    );
}

/// A parent whose key is far longer than the one its children were minted under is followed by
/// none of them: a record naming it copied the whole key into every child's, so that replay held
/// the parent's length times the children. (AT100)
#[test]
fn a_parent_far_longer_than_the_childs_key_is_followed_nowhere() {
    let long = format!("pr-{}", "q".repeat(400));
    let mut rest = vec![at(10, HERE, &[2], "create", 10, written(&long))];
    for n in 0..20u128 {
        let previous = if n == 0 { 10 } else { 19 + n };
        let causes: Vec<u128> = if n == 0 { vec![10] } else { vec![previous, 10] };
        rest.push(at(
            20 + n,
            HERE,
            &causes,
            "create",
            20 + n,
            under(&format!("pr-b.{}", n + 1), 10),
        ));
    }
    let (state, _) = every_way(&base(), &rest);
    for n in 0..20u128 {
        assert_eq!(key(&state, 20 + n), format!("pr-b.{}", n + 1));
    }
}

/// Renames bead makes are followed at every depth, whatever the key: a copy of a key padded with
/// zeros counts from the number without them (`pr-a.0…07` gives way as `pr-a.7`), and a stem with a
/// newline takes a digest (`pr-a\nb` as `pr-a\nbq`), and so does a key ending in a dot, which may
/// then read as numbered (`pr-e.` as `pr-e.3`), each child and grandchild following. (AT100)
#[test]
fn renames_of_unusual_keys_are_followed_at_every_depth() {
    let padded = format!("pr-a.{}7", "0".repeat(24));
    let rest = [
        at(10, THERE, &[2], "create", 10, written(&padded)),
        at(20, HERE, &[2], "create", 20, written(&padded)),
        at(
            21,
            HERE,
            &[20],
            "create",
            21,
            under(&format!("{padded}.1"), 20),
        ),
        at(
            22,
            HERE,
            &[21],
            "create",
            22,
            under(&format!("{padded}.1.1"), 21),
        ),
        at(30, THERE, &[10], "create", 30, written("pr-a\nb")),
        at(40, HERE, &[22], "create", 40, written("pr-a\nb")),
        at(41, HERE, &[40], "create", 41, under("pr-a\nb.1", 40)),
        at(42, HERE, &[41], "create", 42, under("pr-a\nb.1.1", 41)),
        at(50, THERE, &[30], "create", 50, written("pr-e.")),
        at(60, HERE, &[42], "create", 60, written("pr-e.")),
        at(61, HERE, &[60], "create", 61, under("pr-e..1", 60)),
    ];
    let (state, _) = every_way(&base(), &rest);
    for (parent, child) in [(20, 21), (21, 22), (40, 41), (41, 42), (60, 61)] {
        assert_eq!(
            key(&state, child),
            format!("{}.1", key(&state, parent)),
            "{child}"
        );
    }
    assert_ne!(key(&state, 20), padded, "the copy gave way");
}

/// A parent many levels deep whose every level merges counted up (`.1` to `.1000` at each of 72)
/// is followed: the bound grows with the levels of the key the child was minted under. Twice the
/// key's length and 64 bytes, it refused this one. (AT100)
#[test]
fn a_parent_renumbered_at_every_level_is_followed() {
    let minted = format!("ex-base{}", ".1".repeat(72));
    let now = format!("ex-base{}", ".1000".repeat(72));
    let rest = [
        at(10, HERE, &[2], "create", 10, written(&now)),
        at(
            11,
            HERE,
            &[10],
            "create",
            11,
            under(&format!("{minted}.1"), 10),
        ),
    ];
    let (state, _) = every_way(&base(), &rest);
    assert_eq!(key(&state, 11), format!("{now}.1"));
}

/// Under a stem holding a newline every level is no number's, and a copy there takes a digest's
/// prefix as the root's does: a parent whose key took 45 bytes at each of three such levels is
/// followed. Allowed 20 bytes a level and 64 for the root, it was refused. (AT100)
#[test]
fn a_parent_given_a_digest_at_every_level_is_followed() {
    let digest = "q".repeat(45);
    let now = format!("pr-a\nb{digest}.1{digest}.1{digest}");
    let rest = [
        at(10, HERE, &[2], "create", 10, written(&now)),
        at(11, HERE, &[10], "create", 11, under("pr-a\nb.1.1.1", 10)),
    ];
    let (state, _) = every_way(&base(), &rest);
    assert_eq!(key(&state, 11), format!("{now}.1"));
}

/// A create naming a parent it does not name among its own causes follows nowhere, as an older
/// writer's create carrying the member may: one whose writer never saw the parent (its issue one
/// only because a concurrent create's id sorted first), and one whose writer saw it through
/// another event alone. Every writer of the member names it among the causes (AT100).
#[test]
fn a_parent_not_among_the_creates_causes_is_followed_nowhere() {
    let rest = [
        at(10, THERE, &[2], "create", 10, written("pr-q")),
        at(11, HERE, &[2, 10], "create", 11, written("pr-r")),
        at(20, HERE, &[11], "create", 20, under("pr-base.1", 10)),
        at(30, THERE, &[10], "create", 30, under("pr-base.2", 11)),
    ];
    let (state, _) = every_way(&base(), &rest);
    assert_eq!([20, 30].map(|n| key(&state, n)), ["pr-base.1", "pr-base.2"]);
}

/// A parent whose key grew past the bound by merges is not followed, the documented exception:
/// an import keyed with 150 leading zeros, renamed `pr-x.1` in its writer's view, gets its own key
/// back once a merge removes the collision, and the child minted `pr-x.1.1` keeps that key while
/// its parent edge names the parent (`docs/REVIEWING.md` §2). (AT100)
#[test]
fn a_parent_regaining_a_key_past_the_bound_is_not_followed() {
    let padded = format!("pr-x.{}1", "0".repeat(150));
    let rest = [
        at(10, HERE, &[2], "create", 10, written(&padded)),
        at(11, HERE, &[10], "create", 11, under("pr-x.1.1", 10)),
    ];
    let (state, _) = every_way(&base(), &rest);
    assert_eq!(key(&state, 10), padded);
    assert_eq!(key(&state, 11), "pr-x.1.1");
}

/// A child's create naming its parent among its causes beside the head, P made before Q, extends a
/// projection as any write does, one record at a time and in one batch: a cause the projection
/// holds and has placed is in the frontier's past. A projection declined it, and every child made
/// so had the journal replayed (AT100).
#[test]
fn a_create_naming_a_held_parent_extends_the_projection() {
    let rest = [
        at(10, HERE, &[2], "create", 10, written("pr-p")),
        at(11, HERE, &[10], "create", 11, written("pr-q")),
        at(12, HERE, &[11, 10], "create", 12, under("pr-p.1", 10)),
    ];
    let (state, taken) = every_way(&base(), &rest);
    assert_eq!(key(&state, 12), "pr-p.1");
    assert!(taken.in_one_batch, "{taken:?}");
    assert_eq!(taken.declined, 0, "{taken:?}");
}

/// The one journal, read as this build reads it and as a released reader does, which ignores the
/// member: the child of a parent that gave way is `pr-base.2.1` here and `pr-base.1.1` there, its
/// minted key, as *Format and upgrades* in GUARANTEES says. CI runs no released bead, so this
/// holds the difference a mixed fleet shows (AT100).
#[test]
fn an_older_reader_keys_the_child_from_its_minted_key() {
    let mut rest = two_firsts(2);
    rest.push(at(
        22,
        HERE,
        &[21, 20],
        "create",
        22,
        under("pr-base.1.1", 20),
    ));
    let (upgraded, _) = every_way(&base(), &rest);
    let released: Vec<Record> = (rest.iter())
        .map(|record| {
            let mut record = record.clone();
            if let Value::Object(event) = &mut record.value
                && let Some(Value::Object(data)) = event.get_mut("data")
            {
                data.shift_remove("parent");
            }
            record
        })
        .collect();
    let (older, _) = every_way(&base(), &released);
    assert_eq!(key(&upgraded, 22), "pr-base.2.1");
    assert_eq!(key(&older, 22), "pr-base.1.1");
}
