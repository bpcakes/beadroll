//! What every issue's dependency edges answer (roll-lqjj.39), pinned to what the code that held
//! each edge as two strings with a metadata value per addition answered: each issue's edges and
//! the additions behind them with their metadata, in order, readiness, blockers, parents,
//! children, descendants, dependents, cycles, `ready` beneath every issue, and the bytes of every
//! issue's detail (what `bead show` prints) and of the export. Written and recorded before the
//! edges were held typed, and green on the code that held them as strings; checked on every state
//! the differential tests replay (every batch of every seed of their plain, flawed and awaiting
//! histories, as digests in [`ANSWERS`]), on every history the delivery-order property replays,
//! and in full on the journals of stores bead built ([`FIXTURES`]). What a command that reads no
//! texts answers of the edges ([`outline`]) is compared besides between a state read alone and the
//! state it was written from, every time the differential tests read one back.

pub(super) mod drawn;

use crate::id::EventId;
use crate::journal::Journal;
use crate::json::{Format, Object, Value, write};
use crate::query::{Edges, ReadyScope};
use crate::replay::replay;
use crate::state::{Issue, Meta, State};

use super::super::{Extended, decode_state, project};
use super::TextFile;
use super::shown::{FIXTURES as SHOWN, fixture};

/// The digests of every state the differential tests' histories pass through, by shape and seed,
/// as [`fold`] makes them: written by the code that held edges as strings.
const ANSWERS: &str = include_str!("edges/answers");

/// The journals of stores bead built (`<file>\t<record>` lines, as [`fixture`] reads them) and what
/// [`every_way`] gave of them when they were recorded: the two of `shown`, and a tracker of two
/// machines built for this by the bead of 9cbd60a, in which both add the same edge concurrently,
/// an edge is removed by one while the other adds it again and later removed and added again, a
/// cycle is merged from two machines' edges, and two imports of Beads exports bring dependencies of
/// blocks, parent-child, related, discovered-from, waits-for and kinds of their own, with metadata
/// of their own, one waiting for a key the later import brings and one for a key never made.
const FIXTURES: [(&str, &str, &str); 3] = [
    ("golden", SHOWN[0].1, include_str!("edges/golden.answers")),
    ("fleet", SHOWN[1].1, include_str!("edges/fleet.answers")),
    (
        "kinds",
        include_str!("edges/kinds.journal"),
        include_str!("edges/kinds.answers"),
    ),
];

/// An addition's metadata as a value, as the code that held it as one gave it: null while the
/// issue's texts are not read in, which a state replay made never is.
pub(super) fn shown(meta: &Meta) -> Value {
    meta.members()
        .map_or(Value::Null, |members| Value::from(members.clone()))
}

/// A value as compact JSON.
fn text(value: &Value) -> String {
    write(value, Format::COMPACT).expect("a value")
}

/// The keys of `issues`, as one line.
fn keys<'a>(issues: impl IntoIterator<Item = &'a Issue>) -> String {
    let keys: Vec<&str> = issues.into_iter().map(Issue::key).collect();
    keys.join(" ")
}

/// The keys of the issues of `uids` (an id names itself when it is no issue's).
fn named(state: &State, uids: impl IntoIterator<Item = crate::id::EventId>) -> String {
    let keys: Vec<String> = (uids.into_iter())
        .map(|uid| {
            state
                .issue(uid)
                .map_or_else(|| uid.to_string(), |i| i.key().to_owned())
        })
        .collect();
    keys.join(" ")
}

/// What `state` answers of its issues' edges without reading their texts, as `list --parent`,
/// `ready`, `blocked`, `dep tree` and `dep cycles` read them: each edge and the events behind it,
/// readiness and blockers judged for every issue at once, direct blockers, parents, children,
/// descendants, what each waits for, `ready` and cycles.
pub(super) fn outline(state: &State) -> String {
    let mut out = String::new();
    let readiness = state.readiness();
    let lists = state.blocker_lists();
    for issue in state.issues() {
        out.push_str(&format!("{} {}\n", issue.uid(), issue.key()));
        for (edge, _) in issue.deps().iter() {
            let (to, kind) = (edge.to(), edge.kind());
            out.push_str(&format!("  dep {to} {kind}\n"));
        }
        for (edge, tags) in issue.dep_tags() {
            let (to, kind) = (edge.to(), edge.kind());
            let ids: Vec<String> = tags.iter().map(|(id, _)| id.to_string()).collect();
            out.push_str(&format!("  tags {to} {kind} [{}]\n", ids.join(" ")));
        }
        let uid = issue.uid();
        out.push_str(&format!("  blocked {}\n", readiness.is_blocked(issue)));
        out.push_str(&format!("  listed {}\n", keys(lists.of(issue))));
        out.push_str(&format!(
            "  direct {}\n",
            keys(state.direct_blockers(issue))
        ));
        out.push_str(&format!("  parents {}\n", keys(state.parents_of(issue))));
        let children = state.children(uid).iter().copied();
        out.push_str(&format!("  children {}\n", named(state, children)));
        out.push_str(&format!("  waits {}\n", named(state, state.waits_for(uid))));
    }
    out.push_str(&format!("ready {}\n", keys(state.ready(ReadyScope::All))));
    for edges in [Edges::WaitsFor, Edges::Blocking] {
        out.push_str(&format!("cycles {edges:?} {:?}\n", state.cycles(edges)));
    }
    out
}

/// The metadata of every addition behind every edge of every issue of `state`, as text: what a
/// state read back must give once its texts are read in.
pub(super) fn metadata(state: &State) -> String {
    let mut out = String::new();
    for issue in state.issues() {
        for (edge, tags) in issue.dep_tags() {
            let (to, kind) = (edge.to(), edge.kind());
            for (id, meta) in tags.iter() {
                out.push_str(&format!(
                    "{} {to} {kind} {id} {}\n",
                    issue.key(),
                    text(&shown(meta))
                ));
            }
        }
    }
    out
}

/// What `state` answers of its issues' edges with their texts read in: the [`outline`], each
/// edge's metadata and that of each addition behind it, every issue's detail made with all the
/// others' (what `bead show` of several prints), and the export.
pub(super) fn answers(state: &State) -> String {
    let mut out = outline(state);
    let details = state.details_of(state.issues());
    for issue in state.issues() {
        out.push_str(&format!("{}\n", issue.key()));
        for (edge, meta) in issue.deps().iter() {
            let (to, kind) = (edge.to(), edge.kind());
            out.push_str(&format!("  meta {to} {kind} {}\n", text(&shown(meta))));
        }
        for (edge, tags) in issue.dep_tags() {
            let (to, kind) = (edge.to(), edge.kind());
            for (id, meta) in tags.iter() {
                out.push_str(&format!("  tag {to} {kind} {id} {}\n", text(&shown(meta))));
            }
        }
        let detail = Value::from(details.of(issue));
        out.push_str(&format!("  details {}\n", text(&detail)));
    }
    for record in state.export_records() {
        out.push_str(&format!("export {}\n", text(&Value::from(record))));
    }
    out
}

/// The [`answers`], and what is asked of one issue at a time, each issue's ancestry walked anew:
/// whether anything holds it and its blockers, its detail made alone (what `bead show` of one
/// prints), what `ready` lists beneath it, its descendants, and which of the first issues it
/// reaches (what `dep add` refuses a cycle by).
pub(super) fn every_way(state: &State) -> String {
    let mut out = answers(state);
    for issue in state.issues() {
        out.push_str(&format!("{}\n", issue.key()));
        out.push_str(&format!("  held {}\n", state.is_blocked(issue)));
        let uid = issue.uid();
        let beneath = state.descendants(uid);
        out.push_str(&format!("  beneath {}\n", named(state, beneath)));
        let leaf_up = state.subtrees_leaf_up(&[uid]);
        out.push_str(&format!("  leaf-up {}\n", named(state, leaf_up)));
        out.push_str(&format!(
            "  blockers {}\n",
            keys(state.open_blockers(issue))
        ));
        let detail = Value::from(state.detail(issue));
        out.push_str(&format!("  detail {}\n", text(&detail)));
        let ready = state.ready(ReadyScope::Subtree(issue.uid()));
        out.push_str(&format!("  ready {}\n", keys(ready)));
        // of the first issues, whose edges every later one may name: all of them would cost the
        // cube of the issues
        let reached: Vec<&str> = (state.issues().take(8))
            .filter(|other| state.reaches(issue.uid(), other.uid()))
            .map(Issue::key)
            .collect();
        out.push_str(&format!("  reaches {}\n", reached.join(" ")));
    }
    out
}

/// FNV-1a over `bytes`, from `hash`.
fn fnv(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash = (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// The digest of a history before its first state.
pub(super) const START: u64 = 0xcbf2_9ce4_8422_2325;

/// `digest` with what `state` [`answers`] folded in, or for the last state of a history, what it
/// answers [`every_way`].
pub(super) fn fold(digest: u64, state: &State, last: bool) -> u64 {
    let answered = if last {
        every_way(state)
    } else {
        answers(state)
    };
    fnv(digest, answered.as_bytes())
}

/// Fails unless the digests of `shape`'s histories (by seed) are those [`ANSWERS`] records; the
/// message holds their lines as they are now.
pub(super) fn pinned(shape: &str, digests: &[(u64, u64)]) {
    let now: Vec<String> = (digests.iter())
        .map(|(seed, digest)| format!("{shape} {seed} {digest:016x}"))
        .collect();
    let recorded: Vec<&str> = (ANSWERS.lines())
        .filter(|line| line.split(' ').next() == Some(shape))
        .collect();
    assert!(
        !now.is_empty() && now == recorded,
        "the {shape} histories answer otherwise than they did:\n{}",
        now.join("\n")
    );
}

/// Every history the delivery-order property replays (`replay::tests`), each as replay makes the
/// whole of it.
fn delivered(flawed: bool) -> Vec<(u64, u64)> {
    (0..crate::replay::tests::SEEDS)
        .map(|seed| {
            let records = super::history(20_261_016 + seed, crate::replay::tests::BATCHES, flawed);
            let state = replay(super::hostile::journal(&records)).unwrap();
            (seed, fold(START, &state, true))
        })
        .collect()
}

#[test]
fn the_histories_replay_orders_answer_of_their_edges_what_they_did() {
    pinned("delivered", &delivered(false));
    pinned("delivered-flawed", &delivered(true));
}

#[test]
fn the_stores_bead_built_answer_of_their_edges_what_they_did() {
    for (name, journal, recorded) in FIXTURES {
        let state = replay(fixture(journal)).unwrap();
        let now = every_way(&state);
        assert!(
            now == recorded,
            "{name} answers otherwise than it did:\n{now}"
        );
    }
}

/// What `bead export` prints of `state`, and `bead show` of every issue (its detail made alone).
fn printed(state: &State) -> String {
    let mut out = String::new();
    for record in state.export_records() {
        out.push_str(&format!("export {}\n", text(&Value::from(record))));
    }
    for issue in state.issues() {
        out.push_str(&format!(
            "show {}\n",
            text(&Value::from(state.detail(issue)))
        ));
    }
    out
}

#[test]
fn export_and_show_print_the_same_with_edges_metadata_read_from_the_texts_as_from_replay() {
    let mut journals: Vec<Journal> = FIXTURES
        .iter()
        .map(|(_, lines, _)| fixture(lines))
        .collect();
    for flawed in [false, true] {
        for seed in 0..crate::replay::tests::SEEDS {
            let records = super::history(20_261_016 + seed, crate::replay::tests::BATCHES, flawed);
            journals.push(super::hostile::journal(&records));
        }
    }
    let mut additions = 0;
    for journal in journals {
        let replayed = replay(journal.clone()).unwrap();
        let (encoded, _) = project(journal).unwrap().encode(0);
        let file = encoded.texts;
        // a state read alone has every edge and the events behind it, and no metadata
        let mut state = decode_state(&encoded.state).unwrap();
        assert_eq!(outline(&state), outline(&replayed));
        for issue in state.issues() {
            let tags = issue.dep_tags().values();
            assert!(
                tags.flat_map(|tags| tags.values())
                    .all(|meta| meta.members().is_none())
            );
        }
        // `show` reads in the texts of the issues it shows alone
        for issue in replayed.issues().filter(|issue| !issue.deps().is_empty()) {
            let mut one = state.clone();
            super::read_in(&mut one, &file, Some(vec![issue.uid()]));
            let shown = one.detail(one.issue(issue.uid()).unwrap());
            assert_eq!(
                text(&Value::from(shown)),
                text(&replayed.detail(issue).into())
            );
            let read = one.issue(issue.uid()).unwrap();
            assert_eq!(read.dep_tags(), issue.dep_tags());
            additions += issue
                .dep_tags()
                .values()
                .map(|tags| tags.len())
                .sum::<usize>();
        }
        // `export` reads in every issue's
        super::read_in(&mut state, &file, None);
        assert_eq!(printed(&state), printed(&replayed));
        assert_eq!(metadata(&state), metadata(&replayed));
    }
    assert!(additions > 300, "{additions} additions read in");
}

/// A projection whose texts are not read in takes a create of a key an import of one of its issues
/// waits for, in which the create makes an edge whose metadata is with that issue's texts, only once
/// they are read in, as [`Projection::about`] lists them; otherwise it declines, and replay makes
/// the edge.
#[test]
fn a_create_of_an_awaited_key_extends_once_the_waiting_issues_texts_are_read_in() {
    use super::hostile::{HERE, event, id, record};
    let mut init = Object::new();
    init.insert("prefix".into(), Value::from("pr"));
    let create = |key: &str, dangling: Option<Value>| {
        let mut data = Object::new();
        data.insert("key".into(), Value::from(key));
        data.insert("fields".into(), Value::from(Object::new()));
        if let Some(dep) = dangling {
            data.insert("imported".into(), Value::from(true));
            data.insert("dangling".into(), Value::Array(vec![dep]));
        }
        data
    };
    let mut dep = Object::new();
    dep.insert("depends_on_id".into(), Value::from("pr-later"));
    dep.insert("type".into(), Value::from("tracks"));
    dep.insert("metadata".into(), Value::from("{\"why\":\"later\"}"));
    let held = vec![
        record(event(1, HERE, &[], "init", 1, init), HERE, true),
        record(
            event(2, HERE, &[1], "create", 2, create("pr-x", Some(dep.into()))),
            HERE,
            true,
        ),
    ];
    let batch = vec![record(
        event(3, HERE, &[2], "create", 3, create("pr-later", None)),
        HERE,
        true,
    )];
    let mut file = TextFile::new();
    let stored = super::read_back(
        &project(super::hostile::journal(&held)).unwrap(),
        &mut file,
        super::WriteBack::Encoded,
    );
    let waiting = EventId::parse(&id(2)).unwrap();
    assert_eq!(
        stored.about(&batch),
        [EventId::parse(&id(3)).unwrap(), waiting]
    );
    // not read in: declined
    let declined = stored.clone().extend(batch.clone()).unwrap();
    assert!(matches!(declined, Extended::Replay));
    // read in as a store reads them: taken, and replay's answer, the edge's metadata with it
    let mut reading = stored;
    let about = reading.about(&batch);
    super::read_in(reading.state_mut(), &file, Some(about));
    let Extended::Applied(extended) = reading.extend(batch.clone()).unwrap() else {
        panic!("declined with the waiting issue's texts read in");
    };
    let mut all = held;
    all.extend(batch);
    let replayed = replay(super::hostile::journal(&all)).unwrap();
    assert!(super::same(extended.state(), &replayed));
    let read = super::read_back(&extended, &mut file, super::WriteBack::Encoded);
    let mut state = read.into_state();
    super::read_in(&mut state, &file, None);
    assert_eq!(printed(&state), printed(&replayed));
    assert!(printed(&replayed).contains(r#""metadata":"{\"why\":\"later\"}""#));
}

/// Texts whose edges' additions are not the issue's, which only a state changed while its texts
/// were not read in can meet, are refused whole: an addition another event's, as many as there were
/// (an edge removed and added again), an addition more or less, or an edge more.
#[test]
fn texts_whose_edges_differ_from_the_issues_are_never_read_in() {
    let (_, journal, _) = FIXTURES[2];
    let (encoded, _) = project(fixture(journal)).unwrap().encode(0);
    let state = decode_state(&encoded.state).unwrap();
    let file = encoded.texts;
    let edged = (state.issues())
        .find(|issue| issue.dep_tags().values().any(|tags| tags.len() == 1))
        .map(Issue::uid)
        .unwrap();
    let [(uid, place)] = state.wanted_texts([edged])[..] else {
        panic!("its texts are there");
    };
    let bytes = &file[usize::try_from(place.at).unwrap()..][..place.len as usize];
    let later = EventId::from_u128(u128::MAX);
    let changes: [fn(&mut crate::state::DepTags, EventId); 4] = [
        |edges, later| {
            let tags = edges.values_mut().find(|tags| tags.len() == 1).unwrap();
            *tags = crate::state::IdMap::of(later, Meta::default());
        },
        |edges, later| {
            let tags = edges.values_mut().find(|tags| !tags.is_empty()).unwrap();
            tags.insert(later, Meta::default());
        },
        |edges, _| {
            let tags = edges.values_mut().find(|tags| tags.len() == 1).unwrap();
            *tags = crate::state::IdMap::default();
        },
        |edges, later| {
            let edge = crate::state::Edge {
                to: crate::state::Target::Id(later),
                kind: "related".into(),
            };
            edges.insert(edge, crate::state::IdMap::of(later, Meta::default()));
        },
    ];
    for change in changes {
        let mut changed = state.clone();
        change(&mut changed.issues.get_mut(&uid).unwrap().dep_tags, later);
        assert!(changed.read_texts(uid, place, bytes).is_err());
        assert!(
            !changed.issue(uid).unwrap().texts().here(),
            "nothing read in"
        );
    }
    let mut whole = state;
    whole.read_texts(uid, place, bytes).unwrap();
}
