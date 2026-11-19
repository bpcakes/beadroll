//! A child made under its parent's key (`P.n`) follows the parent its create's own parent-child edge
//! names, the next event its writer wrote, when a merge renamed that parent: its key is settled at
//! its create, before anything binds to it, and every way a state is made agrees.

use super::awaiting::{THERE, at, base, creating, every_way, on};
use super::hostile::{HERE, event, id, journal, record};
use super::numbered::{pairs, renames};
use crate::id::EventId;
use crate::journal::JournalOrder;
use crate::json::{Object, Value};
use crate::replay::replay;
use crate::state::State;

use super::super::{Extended, Record, project};

/// A create `bead create` wrote under `key`: not an import's.
fn written(key: &str) -> Object {
    let mut data = creating(key, vec![]);
    data.shift_remove("imported");
    data
}

/// A parent-child edge to the issue numbered `n`, as `bead create --parent` writes it.
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
        at(10, THERE, &[2], "create", 10, written("pr-base.1")),
        at(11, THERE, &[10], "dep_add", 10, parent_of(2)),
        at(20, HERE, &[after], "create", 20, written("pr-base.1")),
        at(21, HERE, &[20], "dep_add", 20, parent_of(2)),
    ]
}

/// A create's own parent-child edge, the next event its writer wrote (`deps` the create alone),
/// names the parent its key was minted under; when a merge renamed that parent the child takes the
/// parent's key now with its number, and its own child follows in turn. Only that edge counts: an
/// import keeps its key, and so does a child whose next event was another, whatever edge a later
/// event adds. Every way of making the state agrees.
#[test]
fn a_child_follows_its_own_parent_edge() {
    let rest = [
        // there: A, `pr-base.1`, sorting first
        at(10, THERE, &[2], "create", 10, written("pr-base.1")),
        at(11, THERE, &[10], "dep_add", 10, parent_of(2)),
        // here: B, minted `pr-base.1` too, and C under it as `pr-base.1.1`
        at(20, HERE, &[2], "create", 20, written("pr-base.1")),
        at(21, HERE, &[20], "dep_add", 20, parent_of(2)),
        at(22, HERE, &[21], "create", 22, written("pr-base.1.1")),
        at(23, HERE, &[22], "dep_add", 22, parent_of(20)),
        // E, whose next event was another: a later edge is not its own
        at(30, HERE, &[23], "create", 30, written("pr-base.1.5")),
        at(31, HERE, &[30], "set", 30, super::hostile::fields("t")),
        at(32, HERE, &[31], "dep_add", 30, parent_of(20)),
        // F, imported, with an edge right after it
        at(
            40,
            HERE,
            &[32],
            "create",
            40,
            creating("pr-base.1.7", vec![]),
        ),
        at(41, HERE, &[40], "dep_add", 40, parent_of(20)),
    ];
    let (state, _) = every_way(&base(), &rest);
    let key = |n: u128| {
        let uid = crate::id::EventId::parse(&id(n)).unwrap();
        state.issue(uid).unwrap().key().to_owned()
    };
    let keys = [10, 20, 22, 30, 40].map(key);
    assert_eq!(
        keys,
        [
            "pr-base.1",
            "pr-base.2",
            "pr-base.2.1",
            "pr-base.1.5",
            "pr-base.1.7"
        ]
    );
    let expected = [("pr-base.1", "pr-base.2"), ("pr-base.1.1", "pr-base.2.1")];
    assert_eq!(renames(&state), pairs(&expected));
}

/// A child's key is settled at its create, its own edge read first, so nothing binds to the key it
/// was minted under and leaves: an import waiting for `pr-base.1.1` binds to A's child E, which
/// holds it, not to C, minted under it and moved to `pr-base.2.1`; E, made between C's create and
/// C's edge, keeps `pr-base.1.1`. An edge another machine writes on its frontier, the create, is
/// not the create's own and moves nothing.
#[test]
fn a_childs_key_is_settled_at_its_create() {
    let rest = [
        at(10, THERE, &[2], "create", 10, written("pr-base.1")),
        at(11, THERE, &[10], "dep_add", 10, parent_of(2)),
        // D, an import waiting for `pr-base.1.1`
        at(
            15,
            HERE,
            &[2],
            "create",
            15,
            creating("pr-imp", vec![on("pr-base.1.1", None, &[])]),
        ),
        at(20, HERE, &[15], "create", 20, written("pr-base.1")),
        at(21, HERE, &[20], "dep_add", 20, parent_of(2)),
        at(22, HERE, &[21], "create", 22, written("pr-base.1.1")),
        // E, A's child, between C's create and C's edge
        at(23, THERE, &[11], "create", 23, written("pr-base.1.1")),
        at(24, HERE, &[22], "dep_add", 22, parent_of(20)),
        at(26, THERE, &[23], "dep_add", 23, parent_of(10)),
        // F, whose next event is another machine's edge
        at(30, HERE, &[24, 26], "create", 30, written("pr-base.1.7")),
        at(31, THERE, &[30], "dep_add", 30, parent_of(20)),
    ];
    let (state, _) = every_way(&base(), &rest);
    assert!(state.integrity().is_empty(), "{:?}", state.integrity());
    let uid = |n: u128| EventId::parse(&id(n)).unwrap();
    let key = |n: u128| state.issue(uid(n)).unwrap().key().to_owned();
    let keys = [10, 20, 22, 23, 30].map(key);
    assert_eq!(
        keys,
        [
            "pr-base.1",
            "pr-base.2",
            "pr-base.2.1",
            "pr-base.1.1",
            "pr-base.1.7"
        ]
    );
    let imported = state.issue(uid(15)).unwrap();
    assert!(imported.dangling().is_empty(), "{:?}", imported.dangling());
    let bound: Vec<String> = (imported.deps().iter())
        .map(|(edge, _)| state.target_key(edge.to()).into_owned())
        .collect();
    assert_eq!(bound, ["pr-base.1.1"]);
}

/// A key a child follows to is passed over when an import waits for it, as an issue's would be:
/// C, minted `pr-base.1.1` under B, follows B to `pr-base.2`, and the import D waits for
/// `pr-base.2.1`, so C is `pr-base.2.2` and D keeps waiting. C took `pr-base.2.1`, and D bound to
/// it.
#[test]
fn a_followed_key_passes_over_one_an_import_waits_for() {
    let mut rest = vec![at(
        15,
        HERE,
        &[2],
        "create",
        15,
        creating("pr-imp", vec![on("pr-base.2.1", None, &[])]),
    )];
    rest.extend(two_firsts(15));
    rest.push(at(22, HERE, &[21], "create", 22, written("pr-base.1.1")));
    rest.push(at(24, HERE, &[22], "dep_add", 22, parent_of(20)));
    let (state, _) = every_way(&base(), &rest);
    assert_eq!(key(&state, 22), "pr-base.2.2");
    let imported = state.issue(EventId::parse(&id(15)).unwrap()).unwrap();
    assert_eq!(
        imported.dangling().len(),
        1,
        "the import bound to the child"
    );
}

/// An edge replay reports and never applies names no parent: C's next event breaks its machine's
/// chain (it does not build on the event before it there), so C keeps `pr-base.1.1`, where it
/// followed B to `pr-base.2.1`.
#[test]
fn an_edge_replay_reports_names_no_parent() {
    let mut rest = two_firsts(2);
    rest.push(at(22, HERE, &[21], "create", 22, written("pr-base.1.1")));
    rest.push(at(23, HERE, &[22], "set", 22, super::hostile::fields("t")));
    rest.push(at(24, HERE, &[22], "dep_add", 22, parent_of(20)));
    let (state, _) = every_way(&base(), &rest);
    assert!(!state.integrity().is_empty(), "the edge applied");
    assert_eq!(key(&state, 22), "pr-base.1.1");
}

/// Two edges that each could be a create's own, attempt records the journal holds unsequenced,
/// settle its key alike in either order they are read in: the smaller id's parent's, B's, so C is
/// `pr-base.2.1`, whether replayed or taken by a projection in one batch, which reads the edges in
/// the order the batch holds them.
#[test]
fn competing_own_edges_settle_alike_in_either_order() {
    let mut rest = two_firsts(2);
    rest.push(at(22, HERE, &[21], "create", 22, written("pr-base.1.1")));
    let attempt = |n: u128, parent: u128| {
        let edge = event(n, HERE, &[22], "dep_add", 22, parent_of(parent));
        record(edge, HERE, JournalOrder::Attempt)
    };
    for edges in [
        [attempt(24, 20), attempt(25, 10)],
        [attempt(25, 10), attempt(24, 20)],
    ] {
        let mut all = base();
        all.extend(rest.iter().cloned());
        all.extend(edges.iter().cloned());
        assert_eq!(key(&replay(journal(&all)).unwrap(), 22), "pr-base.2.1");
        let mut batch = rest.clone();
        batch.extend(edges);
        let Extended::Applied(extended) = project(journal(&base())).unwrap().extend(batch).unwrap()
        else {
            panic!("a projection takes the batch");
        };
        assert_eq!(key(extended.state(), 22), "pr-base.2.1");
    }
}

/// A key with no parent part before its number (`.1`, which no issue's key could have been minted
/// under) keeps its spelling, whatever edge follows its create; it took its parent's key.
#[test]
fn a_key_with_no_parent_part_keeps_its_spelling() {
    let rest = [
        at(10, HERE, &[2], "create", 10, written(".1")),
        at(11, HERE, &[10], "dep_add", 10, parent_of(2)),
    ];
    let (state, _) = every_way(&base(), &rest);
    assert_eq!(key(&state, 10), ".1");
}

/// A child a collision renamed before its own edge applied settles as replay settles it, made every
/// way: the parent P2 gave way to P1 (a stem with a newline, whose copy takes another stem), and C,
/// minted under P2's key, follows it. A projection that applied C's create alone judges the edge by
/// the key C was minted under, not the one its collision gave it.
#[test]
fn a_child_renamed_before_its_edge_settles_as_replay_does() {
    let rest = [
        at(10, THERE, &[2], "create", 10, written("pr-a\nb")),
        at(11, THERE, &[10], "create", 11, written("pr-a\nb.1")),
        at(12, THERE, &[11], "dep_add", 11, parent_of(10)),
        at(20, HERE, &[2], "create", 20, written("pr-a\nb")),
        at(21, HERE, &[20], "create", 21, written("pr-a\nb.1")),
        at(22, HERE, &[21], "dep_add", 21, parent_of(20)),
    ];
    let (state, _) = every_way(&base(), &rest);
    assert_eq!(key(&state, 21), format!("{}.1", key(&state, 20)));
}
