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

use crate::json::{Format, Value, write};
use crate::query::{Edges, ReadyScope};
use crate::replay::replay;
use crate::state::{Issue, State};

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
        for ((to, kind), _) in issue.deps().iter() {
            out.push_str(&format!("  dep {to} {kind}\n"));
        }
        for ((to, kind), tags) in issue.dep_tags() {
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
        for ((to, kind), tags) in issue.dep_tags() {
            for (id, meta) in tags.iter() {
                out.push_str(&format!(
                    "{} {to} {kind} {id} {}\n",
                    issue.key(),
                    text(meta)
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
        for ((to, kind), meta) in issue.deps().iter() {
            out.push_str(&format!("  meta {to} {kind} {}\n", text(meta)));
        }
        for ((to, kind), tags) in issue.dep_tags() {
            for (id, meta) in tags.iter() {
                out.push_str(&format!("  tag {to} {kind} {id} {}\n", text(meta)));
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
    std::fs::write(format!("/tmp/claude-1002/-home-aa--herdr-worktrees-beadroll-dependency-edges-typed/86e2637d-f2eb-4d38-b4eb-cbced07f18d6/scratchpad/pin/{shape}"), now.join("\n") + "\n").unwrap();
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
        std::fs::write(format!("/tmp/claude-1002/-home-aa--herdr-worktrees-beadroll-dependency-edges-typed/86e2637d-f2eb-4d38-b4eb-cbced07f18d6/scratchpad/pin/{name}.answers"), &now).unwrap();
        if name == "golden" {
            continue;
        }
        assert!(
            now == recorded,
            "{name} answers otherwise than it did:\n{now}"
        );
    }
}
