use beadroll_core::journal::JournalOrder;
use beadroll_core::journal::{Journal, read_attempt, read_segment};
use beadroll_core::replay::replay;

use super::*;

fn journal(files: &[Files]) -> Journal {
    let mut journal = Journal::new();
    for machine in files {
        for event in read_segment(&machine.segment, "segment", Some(&mut journal.invalid)) {
            journal.add(event, &machine.replica, "segment", JournalOrder::Sequenced);
        }
    }
    for machine in files {
        for (name, data) in &machine.attempts {
            for event in read_attempt(data, name, Some(&mut journal.invalid)) {
                journal.add(event, &machine.replica, name, JournalOrder::Attempt);
            }
        }
    }
    journal.settle();
    journal
}

#[test]
fn every_event_of_every_shape_applies() {
    for shape in [Shape::Plain, Shape::Ownership, Shape::Churn, Shape::Text] {
        let files = history(3000, shape);
        let journal = journal(&files);
        assert_eq!(journal.events().len(), 3000, "{shape:?}");
        let st = replay(journal).unwrap();
        assert!(st.integrity().is_empty(), "{shape:?}: {:?}", st.integrity());
        assert!(st.pending().is_empty(), "{shape:?}");
        assert!(st.chain_breaks().is_empty(), "{shape:?}");
    }
}

#[test]
fn the_ownership_shape_is_heavy_in_attempts() {
    let files = history(3000, Shape::Ownership);
    let attempted: usize = files
        .iter()
        .flat_map(|machine| &machine.attempts)
        .map(|(_, data)| data.iter().filter(|b| **b == b'\n').count())
        .sum();
    assert!(
        attempted > 1200,
        "{attempted} of 3000 events in attempt files"
    );
    let st = replay(journal(&files)).unwrap();
    let taken_over = st
        .issues()
        .flat_map(|issue| issue.claim_history())
        .filter(|ended| ended.how == "takeover")
        .count();
    assert!(taken_over > 50, "{taken_over} takeovers");
    assert!(
        st.issues()
            .any(|issue| !issue.provisional_claims().is_empty())
    );
}

#[test]
fn the_churn_shape_keeps_its_state_the_size_it_is() {
    let tags = |count: usize| {
        let st = replay(journal(&history(count, Shape::Churn))).unwrap();
        assert_eq!(st.issues().count(), CHURN_ISSUES);
        (st.issues())
            .map(|issue| {
                issue
                    .registers()
                    .values()
                    .map(|tags| tags.len())
                    .sum::<usize>()
            })
            .sum::<usize>()
    };
    let (fewer, more) = (tags(3000), tags(12_000));
    assert!(more < fewer * 2, "{fewer} register values, then {more}");
}

#[test]
fn the_text_shape_is_mostly_text() {
    use beadroll_core::event::Field;
    let st = replay(journal(&history(3000, Shape::Text))).unwrap();
    let text: usize = (st.issues())
        .map(|issue| {
            let fields: usize = [Field::Description, Field::Notes]
                .into_iter()
                .filter_map(|field| issue.get(field).and_then(|value| value.as_str()))
                .map(str::len)
                .sum();
            let comments: usize = (issue.comments().iter())
                .filter_map(|comment| comment.get("text"))
                .filter_map(|text| text.as_str().map(str::len))
                .sum();
            fields + comments
        })
        .sum();
    let issues = st.issues().count();
    // a create's description alone is 1.2 KB at least
    assert!(
        text > issues * 2400,
        "{text} bytes of text on {issues} issues"
    );
}

/// The plain history is the one measured before the ownership shape was added, byte for byte
/// (its digest is pinned), so earlier records stay comparable.
#[test]
fn the_plain_history_is_the_one_always_measured() {
    use sha2::{Digest as _, Sha256};
    let mut digest = Sha256::new();
    for machine in history(2000, Shape::Plain) {
        digest.update(machine.replica.as_bytes());
        digest.update(&machine.segment);
        assert!(machine.attempts.is_empty());
    }
    let hex: String = digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(
        hex,
        "de17de630f0a4b6dfcd50a0797a6a56a93ada20b0f175a1f885ba1549c8dcbfe"
    );
}
