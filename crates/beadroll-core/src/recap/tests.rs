//! The frontier a recap keeps remembers what it showed, no more: on stores that grow as stores
//! take records in (each machine's journal in its own order, attempt files at any time), every
//! event is shown once, in causal order, and none is taken for seen before it was shown, and two
//! reads that saw different parts of a store, their frontiers joined, mark what both placed. Then
//! the cases that make a frontier enough, each on its own.

use std::collections::BTreeSet;

use super::{Window, joined, unseen};
use crate::id::EventId;
use crate::journal::Journal;
use crate::journal::JournalOrder;
use crate::json::{Object, Value};
use crate::order::causal_order;
use crate::projection::Record;
use crate::projection::tests::HistoryKind;
use crate::projection::tests::{Rng, history};
use crate::replay::tests::interleaved;

/// Histories, each taken in by a store a few records at a time.
const SEEDS: u64 = 8;
/// The batches each history's machine 0 takes in while it is written.
const BATCHES: usize = 30;
/// The recaps run on each store while it grows.
const STEPS: usize = 10;

/// Milliseconds of an afternoon in October 2026.
const T: u64 = 1_792_000_000_000;

/// The journal of `records`, taken in in the order of `order`, settled.
fn taken_in(records: &[Record], order: &[usize]) -> Journal {
    let mut journal = Journal::new();
    for record in order.iter().filter_map(|at| records.get(*at)) {
        let record = record.clone();
        journal.add(
            record.value,
            &record.replica,
            &record.origin,
            if record.sequenced {
                JournalOrder::Sequenced
            } else {
                JournalOrder::Attempt
            },
        );
    }
    journal.settle();
    journal
}

#[test]
fn a_growing_store_shows_every_event_once_and_in_causal_order() {
    let mut quarantined = 0;
    for seed in 0..SEEDS {
        for flawed in [false, true] {
            let records = history(
                20_261_017 + seed,
                BATCHES,
                if flawed {
                    HistoryKind::Flawed
                } else {
                    HistoryKind::Valid
                },
            );
            let mut rng = Rng(seed);
            let order = interleaved(&records, &mut rng);
            let mut cuts: Vec<usize> = (0..STEPS).map(|_| rng.below(order.len() + 1)).collect();
            cuts.push(order.len());
            cuts.sort_unstable();
            let (mut frontier, mut before) = (Vec::new(), BTreeSet::new());
            let mut quarantined_before = BTreeSet::new();
            for (step, cut) in cuts.into_iter().enumerate() {
                let at = format!("seed {seed}, flawed {flawed}, step {step}");
                let journal = taken_in(&records, &order[..cut]);
                let (ordered, _) = causal_order(&journal);
                let now: BTreeSet<EventId> = (ordered.iter().copied())
                    .filter(|id| journal.event(*id).is_some_and(|e| e.op_name() != "init"))
                    .collect();
                assert!(before.is_subset(&now), "{at}: an event shown went missing");
                let window = match step {
                    0 => Window::All,
                    _ => Window::After(&frontier),
                };
                let got = unseen(&journal, window);
                assert!(got.placed.is_sorted(), "{at}");
                let placed: BTreeSet<EventId> = ordered.iter().copied().collect();
                assert_eq!(got.placed.iter().copied().collect::<BTreeSet<_>>(), placed);
                // a later read placed all an earlier one did: joining the two adds nothing
                let kept = joined(&got.frontier, &got.placed, &frontier);
                assert_eq!(kept, got.frontier, "{at}");
                let positions: Vec<usize> = (got.events.iter())
                    .map(|id| ordered.iter().position(|o| o == id).unwrap())
                    .collect();
                assert!(positions.is_sorted(), "{at}: not in causal order");
                let shown: BTreeSet<EventId> = got.events.iter().copied().collect();
                assert_eq!(shown.len(), got.events.len(), "{at}: an event shown twice");
                let new: BTreeSet<EventId> = now.difference(&before).copied().collect();
                let hidden: Vec<_> = new.difference(&shown).collect();
                assert!(
                    hidden.is_empty(),
                    "{at}: taken for seen unshown: {hidden:?}"
                );
                let quarantined_now: BTreeSet<EventId> =
                    journal.quarantined().keys().copied().collect();
                let turned = (before.iter())
                    .any(|id| quarantined_now.contains(id) && !quarantined_before.contains(id));
                // an id found quarantined after it was shown may let what it was built on show
                // again (the module's documentation)
                if !turned {
                    assert_eq!(shown, new, "{at}: shown again");
                }
                assert!(got.frontier.iter().all(|id| ordered.contains(id)), "{at}");
                quarantined += quarantined_now.len();
                (frontier, before, quarantined_before) = (got.frontier, now, quarantined_now);
            }
        }
    }
    // the flawed histories reach the one case a frontier treats apart
    assert!(quarantined > 0, "no store held a quarantined id");
}

#[test]
fn two_reads_that_saw_different_events_mark_what_both_placed_once_joined() {
    let (mut exact, mut turned) = (0, 0);
    for seed in 0..SEEDS {
        for flawed in [false, true] {
            let at = format!("seed {seed}, flawed {flawed}");
            let records = history(
                20_261_018 + seed,
                BATCHES,
                if flawed {
                    HistoryKind::Flawed
                } else {
                    HistoryKind::Valid
                },
            );
            let mut rng = Rng(seed);
            // two reads of one store, each part way through its records in an order of its own:
            // each machine's journal read further by one than by the other, as a sync writing
            // file after file lets happen
            let (one, two) = (
                interleaved(&records, &mut rng),
                interleaved(&records, &mut rng),
            );
            let first = taken_in(&records, &one[..rng.below(one.len() + 1)]);
            let second = taken_in(&records, &two[..rng.below(two.len() + 1)]);
            let all = taken_in(&records, &one);
            let (a, b) = (unseen(&first, Window::All), unseen(&second, Window::All));
            let kept = joined(&b.frontier, &b.placed, &a.frontier);
            let later = unseen(&all, Window::After(&kept));
            let both: BTreeSet<EventId> = a.placed.iter().chain(&b.placed).copied().collect();
            let (ordered, _) = causal_order(&all);
            let unread: BTreeSet<EventId> = (ordered.iter().copied())
                .filter(|id| !both.contains(id))
                .filter(|id| all.event(*id).is_some_and(|e| e.op_name() != "init"))
                .collect();
            let shown: BTreeSet<EventId> = later.events.iter().copied().collect();
            let hidden: Vec<_> = unread.difference(&shown).collect();
            assert!(
                hidden.is_empty(),
                "{at}: taken for seen unshown: {hidden:?}"
            );
            // an id quarantined since a read placed it may let what it was built on show again
            let quarantined_since = |read: &Journal, placed: &[EventId]| {
                (placed.iter()).any(|id| {
                    all.quarantined().contains_key(id) && !read.quarantined().contains_key(id)
                })
            };
            if quarantined_since(&first, &a.placed) || quarantined_since(&second, &b.placed) {
                turned += 1;
            } else {
                assert_eq!(shown, unread, "{at}: shown again");
                exact += 1;
            }
        }
    }
    assert!(
        exact > 0 && turned < exact,
        "{exact} exact, {turned} turned"
    );
}

/// The comment `n`, written by `replica` at `millis` on `deps`, saying `text`: its id and record.
fn event(n: u64, millis: u64, replica: &str, deps: &[EventId], text: &str) -> (EventId, Value) {
    let id = format!(
        "{:08x}-{:04x}-7000-8000-{n:012x}",
        millis >> 16,
        millis & 0xffff
    );
    let mut data = Object::new();
    data.insert("text".into(), Value::from(text));
    let mut event = Object::new();
    event.insert("v".into(), Value::from(1));
    event.insert("id".into(), Value::from(id.as_str()));
    event.insert("replica".into(), Value::from(replica));
    let deps = deps.iter().map(|d| Value::from(d.to_string())).collect();
    event.insert("deps".into(), Value::Array(deps));
    event.insert("op".into(), Value::from("comment"));
    event.insert("issue".into(), Value::from(id.as_str()));
    event.insert("data".into(), Value::from(data));
    (EventId::parse(&id).unwrap(), Value::from(event))
}

/// A settled journal of `records`, each in its writer's journal (`replica`), or in one of its
/// attempt files when `attempt`.
fn journal(records: &[(&Value, bool)]) -> Journal {
    let mut journal = Journal::new();
    for (value, attempt) in records {
        let replica = value.get("replica").and_then(Value::as_str).unwrap();
        let origin = if *attempt { "claims" } else { "000001.jsonl" };
        journal.add(
            (*value).clone(),
            replica,
            origin,
            if !attempt {
                JournalOrder::Sequenced
            } else {
                JournalOrder::Attempt
            },
        );
    }
    journal.settle();
    journal
}

#[test]
fn a_linear_history_keeps_its_last_event_and_shows_what_comes_after() {
    let (a, ea) = event(1, T, "0000aa", &[], "a");
    let (b, eb) = event(2, T + 1, "0000aa", &[a], "b");
    let first = unseen(&journal(&[(&ea, false), (&eb, false)]), Window::All);
    assert_eq!((first.events, &first.frontier), (vec![a, b], &vec![b]));
    let (c, ec) = event(3, T + 2, "0000aa", &[b], "c");
    let three = journal(&[(&ea, false), (&eb, false), (&ec, false)]);
    let later = unseen(&three, Window::After(&first.frontier));
    assert_eq!((later.events, later.frontier), (vec![c], vec![c]));
    assert!(unseen(&three, Window::After(&[c])).events.is_empty());
}

#[test]
fn a_frontier_keeps_what_another_read_saw_and_this_one_did_not() {
    let (a, ea) = event(1, T, "0000aa", &[], "a");
    let (b, eb) = event(2, T + 1, "0000aa", &[a], "b");
    let (x, ex) = event(3, T + 2, "0000bb", &[a], "on another machine");
    // one read saw b and not x, the other x and not b: neither frontier is below the other
    let one = unseen(&journal(&[(&ea, false), (&eb, false)]), Window::All);
    let other = unseen(&journal(&[(&ea, false), (&ex, false)]), Window::All);
    assert_eq!(one.placed, [a, b]);
    let kept = joined(&one.frontier, &one.placed, &other.frontier);
    assert_eq!(kept, [b, x]);
    // a read that placed both shows what came after them and keeps its own frontier alone
    let (c, ec) = event(4, T + 3, "0000aa", &[b, x], "c");
    let all = journal(&[(&ea, false), (&eb, false), (&ex, false), (&ec, false)]);
    let later = unseen(&all, Window::After(&kept));
    assert_eq!(later.events, [c]);
    assert_eq!(joined(&later.frontier, &later.placed, &kept), [c]);
}

#[test]
fn an_event_that_arrives_late_shows_whatever_its_clock_said() {
    let (a, ea) = event(1, T, "0000aa", &[], "a");
    let (b, eb) = event(2, T + 10, "0000aa", &[a], "b");
    let first = unseen(&journal(&[(&ea, false), (&eb, false)]), Window::All);
    // written on another machine before b, by a clock a week behind, and synced since
    let (x, ex) = event(3, T - 604_800_000, "0000bb", &[a], "late");
    let all = journal(&[(&ea, false), (&eb, false), (&ex, false)]);
    let later = unseen(&all, Window::After(&first.frontier));
    assert_eq!((later.events, later.frontier), (vec![x], vec![x, b]));
}

#[test]
fn an_event_waiting_for_a_cause_shows_once_the_cause_arrives() {
    let (a, ea) = event(1, T, "0000aa", &[], "a");
    let (b, eb) = event(2, T + 1, "0000bb", &[a], "cause");
    let (c, ec) = event(3, T + 2, "0000bb", &[a, b], "effect");
    let first = unseen(&journal(&[(&ea, false), (&ec, false)]), Window::All);
    assert_eq!((first.events, &first.frontier), (vec![a], &vec![a]));
    let all = journal(&[(&ea, false), (&eb, false), (&ec, false)]);
    let later = unseen(&all, Window::After(&first.frontier));
    assert_eq!((later.events, later.frontier), (vec![b, c], vec![c]));
}

#[test]
fn a_cause_a_quarantined_id_names_is_shown_when_it_arrives_late() {
    let (a, ea) = event(1, T, "0000aa", &[], "a");
    let (x, ex) = event(2, T + 1, "0000bb", &[a], "the late cause");
    let (q, eq) = event(3, T + 2, "0000aa", &[a, x], "one content");
    let (_, other) = event(3, T + 2, "0000aa", &[a, x], "another");
    // q's copies disagree: placed on the cause it names that is here, before x has come
    let first = journal(&[(&ea, false), (&eq, false), (&other, true)]);
    assert!(first.quarantined().contains_key(&q));
    let shown = unseen(&first, Window::All);
    assert_eq!((shown.events, &shown.frontier), (vec![a, q], &vec![a, q]));
    let all = journal(&[(&ea, false), (&ex, false), (&eq, false), (&other, true)]);
    let later = unseen(&all, Window::After(&shown.frontier));
    assert_eq!(later.events, [x], "x was never shown");
}

#[test]
fn a_first_window_shows_what_is_recent_and_counts_the_rest() {
    let (init, mut ei) = event(1, T - 3_000_000, "0000aa", &[], "");
    if let Value::Object(object) = &mut ei {
        object.insert("op".into(), Value::from("init"));
    }
    let (a, ea) = event(2, T - 2_000_000, "0000aa", &[init], "old");
    let (b, eb) = event(3, T, "0000aa", &[a], "recent");
    let all = journal(&[(&ei, false), (&ea, false), (&eb, false)]);
    let window = unseen(&all, Window::Since(T - 1_000_000));
    assert_eq!((window.events, window.earlier), (vec![b], 1));
    assert_eq!(window.frontier, [b]);
    assert_eq!(
        unseen(&all, Window::All).events,
        [a, b],
        "init is never shown"
    );
}
