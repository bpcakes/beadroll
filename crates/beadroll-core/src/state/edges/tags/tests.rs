//! An issue's edges answer what the vector searched in order they were held in before roll-lqjj.21
//! answered, listed or indexed, built by replay or read back and changed after, on seeded draws of
//! additions, removals and lookups; a few edges take a vector and a pointer, and an index is built
//! only once the edges are changed past [`LISTED`].

use std::sync::Arc;

use crate::id::EventId;
use crate::json::{Object, Value};
use crate::projection::tests::Rng;
use crate::work::{Work, counted};

use super::super::super::small::IdMap;
use super::super::{Edge, Meta, Target};
use super::{DepTags, Entry, LISTED, SEARCHES};

/// The edges as they were held before: a vector, searched in order.
#[derive(Clone, Default)]
struct Model(Vec<Entry>);

impl Model {
    fn added(&mut self, edge: Edge) -> &mut IdMap<Meta> {
        let at = match self.0.iter().position(|(was, _)| *was == edge) {
            Some(at) => at,
            None => {
                self.0.push((edge, IdMap::default()));
                self.0.len() - 1
            }
        };
        &mut self.0[at].1
    }

    fn get(&self, to: &Target, kind: &str) -> Option<&IdMap<Meta>> {
        let (_, tags) = self.0.iter().find(|(edge, _)| edge.is(to, kind))?;
        Some(tags)
    }

    fn for_each_to(&mut self, to: &Target, mut change: impl FnMut(&Edge, &mut IdMap<Meta>)) {
        for (edge, tags) in &mut self.0 {
            if edge.to == *to {
                change(edge, tags);
            }
        }
    }
}

/// The id of event `n`.
fn id(n: u64) -> EventId {
    EventId::from_u128((u128::from(n) << 64) | 0x7000_8000_0000_0000)
}

/// Metadata that says which draw made it.
fn meta(n: u64) -> Meta {
    let mut members = Object::new();
    members.insert("n".into(), Value::from(i64::try_from(n).unwrap()));
    Meta::of(members)
}

/// The kinds edges are drawn of, each one `Arc` as replay interns them, and the same texts in
/// `Arc`s of their own, as an extension's batch holds them.
fn kinds() -> Vec<Arc<str>> {
    let names = ["blocks", "parent-child", "related", "", "tracks"];
    (names.iter().map(|name| Arc::from(*name)))
        .chain(names.iter().map(|name| Arc::from(*name)))
        .collect()
}

/// Target `n`: an id, or text that is none, an issue's key or a reference.
fn target_of(n: usize) -> Target {
    match n % 3 {
        0 => Target::Text(format!("ex-{n}").into()),
        1 => Target::Text(format!("jira:OPS-{n}").into()),
        _ => Target::Id(id(1_000 + n as u64)),
    }
}

/// One of the first `targets` targets.
fn target(rng: &mut Rng, targets: usize) -> Target {
    target_of(rng.below(targets))
}

/// What a removal of the edges on a target does to each: when `any`, or when it is of `kind`, it
/// is listed in `seen` and loses its additions before `below`.
fn removal<'a>(
    any: bool,
    kind: &'a str,
    below: EventId,
    seen: &'a mut Vec<Edge>,
) -> impl FnMut(&Edge, &mut IdMap<Meta>) + 'a {
    move |edge, tags| {
        if any || *edge.kind == *kind {
            seen.push(edge.clone());
            tags.retain(|tag, _| *tag >= below);
        }
    }
}

/// Asserts that `edges` answers what `model` does: every edge and its additions in order, and the
/// additions behind every edge on each of `targets` of each of `kinds`.
fn alike(edges: &DepTags, model: &Model, targets: &[Target], kinds: &[Arc<str>], what: &str) {
    let held: Vec<(&Edge, &IdMap<Meta>)> = edges.iter().collect();
    let modelled: Vec<(&Edge, &IdMap<Meta>)> = model.0.iter().map(|(e, t)| (e, t)).collect();
    assert_eq!(held, modelled, "{what}");
    assert_eq!(edges.len(), model.0.len(), "{what}");
    for to in targets {
        for kind in kinds {
            assert_eq!(
                edges.get(to, kind),
                model.get(to, kind),
                "{what}: {to} {kind}"
            );
        }
    }
}

#[test]
fn edges_answer_what_a_vector_searched_in_order_answered_listed_or_indexed() {
    let kinds = kinds();
    let mut indexed = 0;
    for seed in 0..40 {
        let mut rng = Rng(20_261_025 + seed);
        let targets = 1 + rng.below(40);
        let every: Vec<Target> = (0..=targets).map(target_of).collect();
        let (mut edges, mut model) = (DepTags::default(), Model::default());
        for step in 0..200u64 {
            let n = seed * 1_000 + step;
            let to = target(&mut rng, targets);
            let kind = Arc::clone(&kinds[rng.below(kinds.len())]);
            let what = format!("seed {seed}, step {step}");
            let drawn = to.clone();
            match rng.below(10) {
                // added, as dep_add adds it and as bind adds an import's
                0..=4 => {
                    let edge = Edge { to: drawn, kind };
                    edges.added(edge.clone()).insert(id(n), meta(n));
                    model.added(edge).insert(id(n), meta(n));
                }
                5 => {
                    let edge = Edge { to: drawn, kind };
                    let (tag, value) = (id(rng.below(n as usize + 1) as u64), meta(n));
                    edges
                        .added(edge.clone())
                        .insert_if_absent(tag, value.clone());
                    model.added(edge).insert_if_absent(tag, value);
                }
                // removed, of one kind or of any, the additions before a point
                6 | 7 => {
                    let any = rng.below(2) == 0;
                    let below = id(rng.below(n as usize + 1) as u64);
                    let (mut held, mut modelled) = (Vec::new(), Vec::new());
                    edges.for_each_to(&to, removal(any, &kind, below, &mut held));
                    model.for_each_to(&to, removal(any, &kind, below, &mut modelled));
                    assert_eq!(held, modelled, "{what}: the edges removed, in order");
                }
                // read back, as a projection decodes it: listed until its next change
                8 => {
                    edges = DepTags::from_entries(model.0.clone());
                    assert!(
                        !edges.indexed(),
                        "{what}: a state read alone builds no index"
                    );
                }
                // copied, as a state is
                _ => edges = edges.clone(),
            }
            // every target now and then, one never drawn among them; the one drawn every step
            if step % 8 == 7 {
                alike(&edges, &model, &every, &kinds, &what);
                let listed = DepTags::from_entries(model.0.clone());
                assert_eq!(edges, listed, "{what}: alike, indexed or not");
                assert_eq!(format!("{edges:?}"), format!("{listed:?}"), "{what}");
            } else {
                alike(&edges, &model, std::slice::from_ref(&to), &kinds, &what);
            }
            indexed += usize::from(edges.indexed());
        }
    }
    assert!(indexed > 4_000, "{indexed} steps on indexed edges");
}

#[test]
fn a_few_edges_take_a_vector_and_a_pointer_and_are_indexed_at_the_first_change_past_listed() {
    assert_eq!(
        std::mem::size_of::<DepTags>(),
        std::mem::size_of::<Vec<Entry>>() + std::mem::size_of::<usize>()
    );
    let edge = |n: u64| Edge {
        to: Target::Id(id(n)),
        kind: "blocks".into(),
    };
    let mut edges = DepTags::default();
    for n in 0..=LISTED as u64 {
        edges.added(edge(n)).insert(id(n), meta(n));
        assert!(!edges.indexed(), "{} edges", n + 1);
    }
    let read = DepTags::from_entries(edges.iter().map(|(e, t)| (e.clone(), t.clone())).collect());
    edges.added(edge(0)).insert(id(99), meta(99));
    assert!(edges.indexed(), "a change past {LISTED} edges indexes them");
    assert!(!read.indexed(), "reading builds no index");
    assert_eq!(
        read.get(&Target::Id(id(1)), "blocks").map(IdMap::len),
        Some(1)
    );
    // read back, the first change searches in order, and only the second indexes
    let add = |edges: &mut DepTags| {
        edges.added(edge(1));
    };
    let remove = |edges: &mut DepTags| {
        edges.for_each_to(&Target::Id(id(1)), |_, tags| tags.retain(|_, _| false));
    };
    let changes: [&dyn Fn(&mut DepTags); 2] = [&add, &remove];
    for change in changes {
        let mut changed = read.clone();
        change(&mut changed);
        assert!(!changed.indexed(), "a first change after reading");
        change(&mut changed);
        assert!(changed.indexed(), "a second");
    }
}

#[test]
fn an_edge_listed_twice_is_found_at_its_first_place_and_removed_at_both() {
    // only bytes bead did not write hold an edge twice; a lookup finds it where a search in order
    // did: read back, indexed, and once its index has grown, which may place the two in either
    // order, so a hundred indexes, each drawing its own keys
    let edge = Edge {
        to: Target::Text("ex-1".into()),
        kind: "blocks".into(),
    };
    let other = |n: u64| Edge {
        to: Target::Text(format!("ex-other-{n}").into()),
        kind: "related".into(),
    };
    let mut entries: Vec<Entry> = (0..2 * LISTED as u64 + 2)
        .map(|n| (other(n), IdMap::of(id(n), meta(n))))
        .collect();
    entries.insert(1, (edge.clone(), IdMap::default()));
    entries.push((edge.clone(), IdMap::of(id(101), meta(101))));
    let targets = [edge.to.clone(), target_of(0)];
    for round in 0..100 {
        let mut model = Model(entries.clone());
        let mut edges = DepTags::from_entries(entries.clone());
        alike(&edges, &model, &targets, &kinds(), "read back");
        assert_eq!(edges.get(&edge.to, "blocks"), Some(&IdMap::default()));
        for n in 0..=u64::from(SEARCHES) {
            edges.added(edge.clone()).insert(id(102 + n), meta(102 + n));
            model.added(edge.clone()).insert(id(102 + n), meta(102 + n));
        }
        assert!(edges.indexed());
        alike(&edges, &model, &targets, &kinds(), "indexed");
        for n in 200..300 {
            edges.added(other(n)).insert(id(n), meta(n));
            model.added(other(n)).insert(id(n), meta(n));
        }
        alike(
            &edges,
            &model,
            &targets,
            &kinds(),
            &format!("grown, round {round}"),
        );
        let mut visited = 0;
        edges.for_each_to(&edge.to, |_, tags| {
            visited += 1;
            tags.retain(|tag, _| *tag != id(102));
        });
        model.for_each_to(&edge.to, |_, tags| tags.retain(|tag, _| *tag != id(102)));
        assert_eq!(visited, 2);
        alike(&edges, &model, &targets, &kinds(), "both removed");
    }
}

#[test]
fn an_edge_is_found_and_added_by_looking_at_the_edges_on_its_target_alone() {
    // the work guards count this on replay (`work::tests::degree`); here, the edge set alone
    let edges_of = |n: u64| {
        let mut edges = DepTags::default();
        for at in 0..n {
            let to = Target::Text(format!("ex-{at}").into());
            edges
                .added(Edge {
                    to,
                    kind: "blocks".into(),
                })
                .insert(id(at), meta(at));
        }
        edges
    };
    let (edges, work) = counted(|| edges_of(4_000));
    let probes = work.of(Work::Probe);
    assert!(
        (4_000..4 * 4_000).contains(&probes),
        "{probes} edges looked at"
    );
    let (_, work) = counted(|| {
        for at in 0..4_000 {
            let to = Target::Text(format!("ex-{at}").into());
            assert!(edges.get(&to, "blocks").is_some());
        }
    });
    let probes = work.of(Work::Probe);
    assert!((2 * 4_000..3 * 4_000).contains(&probes), "{probes} looks");
}
