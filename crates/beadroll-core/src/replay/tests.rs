//! Replay is invariant under delivery order (roll-k2wy.14.4): the records of a generated history,
//! given to a journal in any order their causes allow, or as a store reads them, each machine's
//! journal in its own order and the attempt files after them, replay to the same state with the
//! same answers. The histories come from the differential tests' generator (`projection::tests`,
//! with seeds and lengths of their own): three machines that write, sync and race, colliding keys
//! and all, with claims and releases in attempt files, takeovers and imports that wait for a key.
//! The answers are asked through the public API alone: every field of every issue
//! (`Issue::field`), the views, export, readiness and blockers, cycles, conflicts, and what the
//! state says of its events.

use std::collections::BTreeSet;

use super::replay;
use crate::event::Field;
use crate::id::EventId;
use crate::journal::Journal;
use crate::journal::JournalOrder;
use crate::json::{self, Format, Object, Value};
use crate::projection::Record;
use crate::projection::tests::HistoryKind;
use crate::projection::tests::awaiting::fnv;
use crate::projection::tests::owned::{self, BASIS};
use crate::projection::tests::{Rng, history};
use crate::query::{Edges, ReadyScope};
use crate::state::State;

/// Histories, each delivered in several orders.
pub(crate) const SEEDS: u64 = 12;
/// The batches machine 0's store takes in while a history is written: about 160 records each.
pub(crate) const BATCHES: usize = 30;
/// Orders drawn of each history, of each kind.
const ORDERS: usize = 6;

/// What `state` answers, in order, each answer with what was asked; with the records that could not
/// be used sorted, when `flawed`.
fn answers(state: &State, flawed: HistoryKind) -> Vec<(String, String)> {
    let flawed = matches!(flawed, HistoryKind::Flawed);
    let mut said: Vec<(String, String)> = Vec::new();
    let mut say = |asked: String, answer: String| said.push((asked, answer));
    let shown = |object: Object| written(&Value::from(object));
    say("prefix".into(), format!("{:?}", state.prefix()));
    say("name".into(), format!("{:?}", state.name()));
    say("keys".into(), format!("{:?}", state.keys()));
    let readiness = state.readiness();
    let blockers = state.blocker_lists();
    let details = state.details_of(state.issues());
    for issue in state.issues() {
        let key = issue.key();
        for field in Field::ALL {
            say(
                format!("{key} {field:?}"),
                format!("{:?}", issue.field(field)),
            );
        }
        say(format!("{key} uid"), issue.uid().to_string());
        say(format!("{key} labels"), format!("{:?}", issue.labels()));
        say(format!("{key} brief"), shown(state.brief(issue)));
        say(format!("{key} detail"), shown(state.detail(issue)));
        say(format!("{key} details_of"), shown(details.of(issue)));
        say(format!("{key} line"), state.line(issue));
        say(
            format!("{key} conflicts"),
            format!("{:?}", state.conflicts(issue)),
        );
        say(format!("{key} claim"), format!("{:?}", issue.claim()));
        say(
            format!("{key} claims"),
            format!(
                "{:?} {:?} {:?} {:?}",
                issue.claim_history(),
                issue.provisional_claims(),
                issue.lost_claims(),
                issue.stale()
            ),
        );
        let blocked = (readiness.is_blocked(issue), state.is_blocked(issue));
        say(format!("{key} blocked"), format!("{blocked:?}"));
        let keys = |issues: Vec<&crate::state::Issue>| -> Vec<String> {
            issues.iter().map(|issue| issue.key().to_owned()).collect()
        };
        say(
            format!("{key} blockers"),
            format!(
                "{:?} {:?}",
                keys(blockers.of(issue)),
                keys(state.open_blockers(issue))
            ),
        );
        say(
            format!("{key} children"),
            format!("{:?}", state.children(issue.uid())),
        );
    }
    let ready: Vec<&str> = (state.ready(ReadyScope::All).iter())
        .map(|issue| issue.key())
        .collect();
    say("ready".into(), format!("{ready:?}"));
    for edges in [Edges::WaitsFor, Edges::Blocking] {
        say(
            format!("cycles {edges:?}"),
            format!("{:?}", state.cycles(edges)),
        );
    }
    let export: Vec<String> = state.export_records().into_iter().map(shown).collect();
    say("export".into(), export.join("\n"));
    say("renames".into(), format!("{:?}", state.renames()));
    let mut integrity: Vec<String> = (state.integrity().iter())
        .map(|row| format!("{row:?}"))
        .collect();
    if flawed {
        integrity.sort();
    }
    say("integrity".into(), integrity.join("\n"));
    let pending: BTreeSet<EventId> = state.pending().iter().copied().collect();
    say("pending".into(), format!("{pending:?}"));
    say("orphans".into(), state.orphans().to_string());
    say("comments".into(), state.comment_count().to_string());
    say("by thread".into(), format!("{:?}", state.last_by_thread()));
    say("chain breaks".into(), format!("{:?}", state.chain_breaks()));
    // The events, in their journal's order by design: by id here, with who wrote each and what
    // each had seen of the ten after it.
    let events: Vec<EventId> = state
        .events()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    for (at, event) in events.iter().enumerate() {
        let seen: Vec<bool> = (events.iter().skip(at + 1).take(10))
            .map(|other| state.saw(*event, *other))
            .collect();
        say(
            format!("event {event}"),
            format!("{} {seen:?}", state.who(*event)),
        );
    }
    said
}

fn written(value: &Value) -> String {
    json::write(value, Format::CANONICAL).unwrap_or_else(|error| error.to_string())
}

/// The state replay makes of `records`, given to a journal in the order of `order`.
fn delivered(records: &[Record], order: &[usize]) -> State {
    let mut journal = Journal::new();
    for at in order {
        let record = &records[*at];
        journal.add(
            record.value.clone(),
            &record.replica,
            &record.origin,
            if record.sequenced {
                JournalOrder::Sequenced
            } else {
                JournalOrder::Attempt
            },
        );
    }
    replay(journal).unwrap()
}

/// The ids of the events `record` names as its causes.
fn causes(record: &Record) -> Vec<String> {
    (record
        .value
        .get("deps")
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter())
    .filter_map(|dep| dep.as_str().map(str::to_owned))
    .collect()
}

/// An order of `records` their causes allow: each record after every record of an event it names,
/// the next drawn at random from those whose causes have all come.
fn causal(records: &[Record], rng: &mut Rng) -> Vec<usize> {
    let ids: Vec<String> = (records.iter())
        .map(|record| {
            record
                .value
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    let mut waiting: Vec<BTreeSet<String>> = (records.iter())
        .map(|record| {
            (causes(record).into_iter())
                .filter(|cause| ids.contains(cause))
                .collect()
        })
        .collect();
    let mut left: Vec<usize> = (0..records.len()).collect();
    let mut order = Vec::with_capacity(records.len());
    while !left.is_empty() {
        let free: Vec<usize> = (0..left.len())
            .filter(|at| waiting[left[*at]].is_empty())
            .collect();
        assert!(!free.is_empty(), "a history whose causes loop");
        let next = left.remove(free[rng.below(free.len())]);
        for wait in &mut waiting {
            wait.remove(&ids[next]);
        }
        order.push(next);
    }
    order
}

/// An order a store can read `records` in, which need not follow their causes: each machine's
/// journal in its own order, the journals interleaved at random, and each attempt file anywhere.
/// Also the order a growing store takes them in (`recap::tests`).
pub(crate) fn interleaved(records: &[Record], rng: &mut Rng) -> Vec<usize> {
    let mut queues: Vec<Vec<usize>> = Vec::new();
    let mut folders: Vec<&str> = Vec::new();
    for (at, record) in records.iter().enumerate() {
        if !record.sequenced {
            queues.push(vec![at]);
            folders.push("");
            continue;
        }
        match folders.iter().position(|folder| *folder == record.replica) {
            Some(queue) => queues[queue].push(at),
            None => {
                queues.push(vec![at]);
                folders.push(&record.replica);
            }
        }
    }
    let mut order = Vec::with_capacity(records.len());
    while !queues.is_empty() {
        let queue = rng.below(queues.len());
        order.push(queues[queue].remove(0));
        if queues[queue].is_empty() {
            queues.remove(queue);
        }
    }
    order
}

/// As a store reads `records`: every machine's journal in turn, by its folder, then the attempt
/// files by name.
fn stored(records: &[Record]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..records.len()).collect();
    order.sort_by_key(|at| {
        let record = &records[*at];
        let file = if record.sequenced {
            String::new()
        } else {
            record.origin.clone()
        };
        (!record.sequenced, record.replica.clone(), file)
    });
    order
}

/// The first answer `got` gives otherwise than `expected`, with what was asked.
fn first_difference(expected: &[(String, String)], got: &[(String, String)]) -> Option<String> {
    let differs = expected.iter().zip(got).find(|(a, b)| a != b);
    match differs {
        Some(((asked, wanted), (_, given))) => Some(format!(
            "{asked}:\n  in order written: {wanted}\n  delivered: {given}"
        )),
        None if expected.len() != got.len() => {
            Some(format!("{} answers, then {}", expected.len(), got.len()))
        }
        None => None,
    }
}

/// What [`delivered_alike`] delivered: the most machines a history had, the records, those of
/// attempt files, the copies renamed and what replay reported; and the digest of what replay
/// answers of every issue's ownership in each history (`owned::answers`).
#[derive(Default)]
struct Tally {
    machines: usize,
    records: usize,
    attempts: usize,
    renamed: usize,
    reported: usize,
    owned: u64,
}

/// Replays each history of `seeds` in the order written and in other orders, and fails at the
/// first answer that differs. A history `flawed` also holds what replay reports, among it a
/// machine whose identity was copied writing to another's journal: its records are delivered only
/// in orders that keep each journal's own order, a journal's order being its file's, and the
/// records that could not be used are compared as a set, since replay reports them in the order
/// found.
fn delivered_alike(seeds: std::ops::Range<u64>, history_kind: HistoryKind) -> Tally {
    let flawed = matches!(history_kind, HistoryKind::Flawed);
    let mut tally = Tally {
        owned: BASIS,
        ..Tally::default()
    };
    for seed in seeds {
        let records = history(20_261_016 + seed, BATCHES, history_kind);
        tally.records += records.len();
        let machines: BTreeSet<&str> = records.iter().map(|r| r.replica.as_str()).collect();
        tally.machines = tally.machines.max(machines.len());
        tally.attempts += records.iter().filter(|record| !record.sequenced).count();
        let written: Vec<usize> = (0..records.len()).collect();
        let first = delivered(&records, &written);
        tally.renamed += first.renames().len();
        tally.reported += first.integrity().len();
        tally.owned = fnv(tally.owned, owned::answers(&first).as_bytes());
        let expected = answers(&first, history_kind);
        let mut rng = Rng(seed);
        let mut orders = vec![("as a store reads", stored(&records))];
        for _ in 0..ORDERS {
            if !flawed {
                orders.push(("in causal order", causal(&records, &mut rng)));
            }
            orders.push(("interleaved", interleaved(&records, &mut rng)));
        }
        for (kind, order) in orders {
            let got = answers(&delivered(&records, &order), history_kind);
            if let Some(difference) = first_difference(&expected, &got) {
                panic!("seed {seed}, {kind}: {difference}");
            }
        }
    }
    tally
}

#[test]
fn replay_answers_alike_in_any_order_the_causes_allow_and_as_a_store_reads() {
    let tally = delivered_alike(0..SEEDS, HistoryKind::Valid);
    // The histories hold what the property is about: events of three machines, attempt files,
    // and copies two machines minted alike, renamed; and nothing replay reports.
    assert_eq!(tally.machines, 3, "records of {} machines", tally.machines);
    assert!(tally.records > 1_500, "{} records", tally.records);
    assert!(tally.attempts > 200, "{} attempt records", tally.attempts);
    assert!(tally.renamed > 100, "{} renames", tally.renamed);
    assert_eq!(tally.reported, 0, "{} reported", tally.reported);
    assert_eq!(tally.owned, owned::DELIVERED, "{:#x}", tally.owned);
}

#[test]
fn replay_of_what_it_reports_answers_alike_in_any_order_that_keeps_each_journals() {
    let tally = delivered_alike(0..SEEDS, HistoryKind::Flawed);
    assert!(tally.records > 1_500, "{} records", tally.records);
    assert!(tally.reported > 200, "{} reported", tally.reported);
    assert_eq!(tally.owned, owned::DELIVERED_FLAWED, "{:#x}", tally.owned);
}
