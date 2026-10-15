//! A projection's finishing pass (roll-lqjj.17): a batch relabels the issues it changes the labels
//! of, each once, and builds the parent-child index only when it changes an edge.

use crate::json::{Object, Value};
use crate::projection::{Projection, Record};

use super::super::Work;
use super::{History, SMALL, applied, grows_linearly, key, one_at_a_time};

/// A tracker of `issues` issues, each with two labels and, unless it is the first of its ten, a
/// parent in that first, which is the shape `timing --extend` measures: its history, and the
/// number of the event that created each issue, in order.
fn labelled(issues: usize) -> (History, Vec<u64>) {
    let mut history = History::new();
    let created = (1..=issues)
        .map(|n| {
            let first = n - (n - 1) % 10;
            let parent: Vec<(String, &str)> = (n != first)
                .then(|| (key(first), "parent-child"))
                .into_iter()
                .collect();
            history.create(&key(n), &parent, &["ui", "db"])
        })
        .collect();
    (history, created)
}

/// The labels `labels` name.
fn labels(labels: &[&str]) -> Object {
    let mut data = Object::new();
    let labels = labels.iter().map(|label| Value::from(*label)).collect();
    data.insert("labels".into(), Value::Array(labels));
    data
}

/// What [`labelled`] makes of `issues`, as a projection, and the records of one event doing `op`
/// with `data` made from its number on each issue, after it.
fn each_issue(
    issues: usize,
    op: &str,
    data: impl Fn(usize) -> Object,
) -> (Projection, Vec<Record>) {
    let (mut history, created) = labelled(issues);
    let held = history.events.len();
    let projection = history.projection();
    for (n, issue) in created.into_iter().enumerate() {
        history.add(op, Some(issue), data(n));
    }
    (projection, history.after(held))
}

#[test]
fn closes_applied_one_at_a_time_derive_no_labels_and_no_index() {
    grows_linearly(
        "closing every issue one event at a time",
        &[Work::Relabel, Work::Index],
        |issues| each_issue(issues, "close", |_| Object::new()),
        |(projection, closes)| drop(one_at_a_time(projection, closes)),
    );
}

#[test]
fn label_additions_applied_one_at_a_time_relabel_their_own_issues_alone() {
    grows_linearly(
        "adding a label to every issue one event at a time",
        &[Work::Relabel, Work::Index],
        |issues| each_issue(issues, "label_add", |n| labels(&[&format!("l{n}")])),
        |(projection, additions)| drop(one_at_a_time(projection, additions)),
    );
}

#[test]
fn a_batch_that_changes_one_issues_labels_over_and_over_relabels_it_once() {
    let build = |additions: usize| {
        let (mut history, created) = labelled(SMALL);
        let held = history.events.len();
        let projection = history.projection();
        for n in 0..additions {
            history.add("label_add", Some(created[0]), labels(&[&format!("c{n}")]));
        }
        (projection, history.after(held))
    };
    grows_linearly(
        "adding labels to one issue in one batch",
        &[Work::Relabel],
        build,
        |(projection, additions)| drop(applied(projection, additions)),
    );
}
