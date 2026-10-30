//! `timing --renames N`: what replay takes when N copies are renamed (roll-lqjj.25, X03) — two
//! machines each create the issues `ex-1` to `ex-N`, as two machines importing one export before
//! they sync do, so that the second machine's N creates each find their key taken and are renamed
//! under a key neither an issue nor an import has. Its control is the same 2N creates under 2N
//! different keys, renamed by nobody. Each is also measured with every first-machine issue
//! importing a dependency on a key that never arrives, so that the claimed keys are the issues'
//! and the keys waited for together. Choosing a copy's key used to collect every key claimed for
//! each copy, so replay with copies renamed grew with the square of N while its control grew with
//! N. The same four are measured on numbered keys (roll-lqjj.46): both machines create the
//! children `ex-1.1` to `ex-1.N`, each copy counting upward past every number taken, and the keys
//! the first machine's issues wait for are the next N numbers, `ex-1.N+1` to `ex-1.2N`, which the
//! copies must count past too. Counting past every number taken one number at a time made N copies
//! ask about N² keys. A fifth numbered shape has the first machine create every other number, so
//! that each copy takes the number after its own and the numbers taken make N runs of one, among
//! which every copy looks its own up. Each numbered control creates, under the second machine, the
//! keys its copies end up with. The fastest of [`RUNS`] runs counts. Evidence, never a gate.

use std::time::Instant;

use beadroll_core::journal::Journal;
use beadroll_core::journal::JournalOrder;
use beadroll_core::json::{Object, Value};
use beadroll_core::replay::replay;
use beadroll_core::state::State;

use super::world::profile;
use crate::workspace::{Failure, json_string};

/// How many times each journal is replayed; the fastest run counts.
const RUNS: usize = 3;

/// The two machines whose journals the probe's events are in.
const MACHINES: [&str; 2] = ["a", "b"];

/// What the two machines create.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Shape {
    /// Both create `ex-1` to `ex-N`: the second machine's N copies are renamed.
    Renamed,
    /// The first creates `ex-1` to `ex-N`, the second `ex-N+1` to `ex-2N`: nothing is renamed.
    Control,
    /// [`Shape::Renamed`], every first-machine issue also importing a dependency on a key that
    /// never comes.
    RenamedWaiting,
    /// [`Shape::Control`], with the same dependencies on keys that never come.
    ControlWaiting,
    /// Both create `ex-1.1` to `ex-1.N`: the second machine's N copies count upward, to `ex-1.N+1`
    /// to `ex-1.2N`.
    Numbered,
    /// The first creates `ex-1.1` to `ex-1.N`, the second `ex-1.N+1` to `ex-1.2N`, the keys
    /// [`Shape::Numbered`]'s copies end up with: nothing is renamed.
    NumberedControl,
    /// [`Shape::Numbered`], first-machine issue n also importing a dependency on `ex-1.N+n`, which
    /// never comes: the copies count past those too, to `ex-1.2N+1` to `ex-1.3N`.
    NumberedWaiting,
    /// The same first machine, the second creating `ex-1.2N+1` to `ex-1.3N`, the keys
    /// [`Shape::NumberedWaiting`]'s copies end up with: nothing is renamed, and nothing arrives.
    NumberedControlWaiting,
    /// Both create every other number, `ex-1.1`, `ex-1.3` to `ex-1.2N-1`: each copy takes the
    /// even number after its own, so that the numbers copies find taken are N runs of one.
    NumberedGaps,
    /// The first machine of [`Shape::NumberedGaps`], the second creating the even numbers its
    /// copies end up with: nothing is renamed.
    NumberedGapsControl,
}

impl Shape {
    const ALL: [Shape; 10] = [
        Shape::Renamed,
        Shape::Control,
        Shape::RenamedWaiting,
        Shape::ControlWaiting,
        Shape::Numbered,
        Shape::NumberedControl,
        Shape::NumberedWaiting,
        Shape::NumberedControlWaiting,
        Shape::NumberedGaps,
        Shape::NumberedGapsControl,
    ];

    /// The name its figure is recorded under.
    fn name(self) -> &'static str {
        match self {
            Shape::Renamed => "renamed",
            Shape::Control => "control",
            Shape::RenamedWaiting => "renamed_waiting",
            Shape::ControlWaiting => "control_waiting",
            Shape::Numbered => "numbered",
            Shape::NumberedControl => "numbered_control",
            Shape::NumberedWaiting => "numbered_waiting",
            Shape::NumberedControlWaiting => "numbered_control_waiting",
            Shape::NumberedGaps => "numbered_gaps",
            Shape::NumberedGapsControl => "numbered_gaps_control",
        }
    }

    /// Whether the second machine creates the first's keys.
    fn copies(self) -> bool {
        matches!(
            self,
            Shape::Renamed
                | Shape::RenamedWaiting
                | Shape::Numbered
                | Shape::NumberedWaiting
                | Shape::NumberedGaps
        )
    }

    /// Whether the first machine's issues import a dependency on a key that never comes.
    fn waits(self) -> bool {
        matches!(
            self,
            Shape::RenamedWaiting
                | Shape::ControlWaiting
                | Shape::NumberedWaiting
                | Shape::NumberedControlWaiting
        )
    }

    /// Whether its keys are the numbered children of `ex-1`.
    fn numbered(self) -> bool {
        matches!(
            self,
            Shape::Numbered
                | Shape::NumberedControl
                | Shape::NumberedWaiting
                | Shape::NumberedControlWaiting
                | Shape::NumberedGaps
                | Shape::NumberedGapsControl
        )
    }

    /// Whether the first machine creates every other number.
    fn gaps(self) -> bool {
        matches!(self, Shape::NumberedGaps | Shape::NumberedGapsControl)
    }

    /// Key number `n`: `ex-n`, or `ex-1.n` when numbered.
    fn key(self, n: usize) -> String {
        if self.numbered() {
            format!("ex-1.{n}")
        } else {
            format!("ex-{n}")
        }
    }

    /// The key machine `turn`'s create number `n` of `issues` names: the first's own numbers; the
    /// second's the same when it copies them, and otherwise the numbers after the first's and,
    /// when numbered keys wait, after the keys they wait for too; with gaps, the first's are the
    /// odd numbers and the second's control the even ones.
    fn named(self, turn: usize, n: usize, issues: usize) -> String {
        if self.gaps() {
            let odd = turn == 0 || self.copies();
            return self.key(if odd { 2 * n - 1 } else { 2 * n });
        }
        let skipped = if self.numbered() && self.waits() {
            issues
        } else {
            0
        };
        let number = if turn == 0 || self.copies() {
            n
        } else {
            issues + skipped + n
        };
        self.key(number)
    }

    /// The key first-machine issue `n` of `issues` waits for, which never comes: `ex-gone-n`, or
    /// when numbered `ex-1.N+n`, among the numbers the copies count past.
    fn awaited(self, n: usize, issues: usize) -> String {
        if self.numbered() {
            self.key(issues + n)
        } else {
            format!("ex-gone-{n}")
        }
    }
}

/// The id of the probe's event number `n`: a UUIDv7, ids in the order of their numbers.
fn id(n: u64) -> String {
    let millis = 1_791_558_245_123 + n;
    format!(
        "{:08x}-{:04x}-7000-8000-{:012x}",
        millis >> 16,
        millis & 0xffff,
        n
    )
}

/// Event `n` of `machine`, doing `op` on itself, built on `after`, as the events of one machine's
/// journal are.
fn event(n: u64, machine: &str, after: Option<u64>, op: &str, data: Object) -> Value {
    let mut actor = Object::new();
    actor.insert("agent".into(), Value::from("timing"));
    actor.insert("thread".into(), Value::from(machine));
    let mut members = Object::new();
    members.insert("v".into(), Value::from(1));
    members.insert("id".into(), Value::from(id(n)));
    members.insert("replica".into(), Value::from(machine));
    let deps: Vec<Value> = after
        .map(|after| Value::from(id(after)))
        .into_iter()
        .collect();
    members.insert("deps".into(), Value::Array(deps));
    members.insert("actor".into(), Value::from(actor));
    members.insert("op".into(), Value::from(op));
    members.insert("issue".into(), Value::from(id(n)));
    members.insert("data".into(), Value::from(data));
    Value::from(members)
}

/// The create of an imported issue under `key`, importing a dependency on `awaited` when given.
fn create(key: &str, awaited: Option<String>) -> Object {
    let mut fields = Object::new();
    fields.insert("title".into(), Value::from(format!("issue {key}")));
    let mut data = Object::new();
    data.insert("key".into(), Value::from(key));
    data.insert("fields".into(), Value::from(fields));
    data.insert("imported".into(), Value::from(true));
    if let Some(awaited) = awaited {
        let mut dep = Object::new();
        dep.insert("depends_on_id".into(), Value::from(awaited));
        dep.insert("type".into(), Value::from("blocks"));
        data.insert("dangling".into(), Value::Array(vec![Value::from(dep)]));
    }
    data
}

/// The journal of `shape` with `issues` creates a machine, after an `init` of the first machine's:
/// the first machine's creates in a chain after it, the second's in a chain of its own after it.
pub(super) fn journal(shape: Shape, issues: usize) -> Journal {
    let mut journal = Journal::new();
    let mut data = Object::new();
    data.insert("prefix".into(), Value::from("ex"));
    journal.add(
        event(1, MACHINES[0], None, "init", data),
        MACHINES[0],
        "",
        JournalOrder::Sequenced,
    );
    for (turn, machine) in MACHINES.iter().enumerate() {
        for n in 1..=issues {
            let number = turn * issues + n;
            let id = number as u64 + 1;
            let after = if n > 1 { id - 1 } else { 1 };
            let awaited = (shape.waits() && turn == 0).then(|| shape.awaited(n, issues));
            let create = create(&shape.named(turn, n, issues), awaited);
            journal.add(
                event(id, machine, Some(after), "create", create),
                machine,
                "",
                JournalOrder::Sequenced,
            );
        }
    }
    journal
}

/// The fastest of [`RUNS`] replays of `journal`, copied before the clock runs, and the state the
/// last one made.
fn fastest(journal: &Journal) -> Result<(f64, State), Failure> {
    let mut best = f64::INFINITY;
    let mut last = None;
    for _ in 0..RUNS {
        let trial = journal.clone();
        let clock = Instant::now();
        let state = replay(trial).map_err(|_| Failure::Failed("the probe needs a key".into()))?;
        best = best.min(clock.elapsed().as_secs_f64());
        last = Some(state);
    }
    let state = last.ok_or_else(|| Failure::Failed("the probe replays at least once".into()))?;
    Ok((best, state))
}

/// How many dependencies on keys that never come the state's issues still wait with.
fn waiting(state: &State) -> usize {
    state.issues().map(|issue| issue.dangling().len()).sum()
}

/// Measures; returns the beginning of the line to record.
pub(crate) fn measure(issues: usize, label: Option<&str>) -> Result<String, Failure> {
    let mut figures = String::new();
    for shape in Shape::ALL {
        let (seconds, state) = fastest(&journal(shape, issues))?;
        let name = shape.name();
        figures.push_str(&format!(
            "\"{name}_seconds\": {seconds:.4}, \"{name}_issues\": {}, \"{name}_renamed\": {}, \
             \"{name}_waiting\": {}, ",
            state.issues().count(),
            state.renames().len(),
            waiting(&state),
        ));
    }
    let label = label.map_or_else(
        || {
            format!(
                "replay of {issues} copies renamed, of plain and of numbered keys, and of the same \
                 creates renamed by nobody"
            )
        },
        str::to_owned,
    );
    Ok(format!(
        "{{\"label\": {}, \"copies\": {issues}, \"profile\": {}, \"runs\": {RUNS}, {figures}",
        json_string(&label),
        json_string(profile()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The state replay makes of `issues` creates a machine of `shape`.
    fn replayed(shape: Shape, issues: usize) -> State {
        replay(journal(shape, issues)).unwrap()
    }

    #[test]
    fn the_second_machines_copies_are_renamed_under_keys_of_their_own() {
        let state = replayed(Shape::Renamed, 6);
        assert_eq!(state.issues().count(), 12);
        assert_eq!(state.renames().len(), 6);
        // a key with no number is extended by the shortest free prefix of the copy's digest
        for rename in state.renames() {
            assert!(rename.from.starts_with("ex-") && rename.to.starts_with(&rename.from));
            assert_eq!(rename.replica, "b");
            assert!(state.claimed_keys().contains(rename.to.as_str()));
        }
        let keys: std::collections::HashSet<&str> = state.issues().map(|i| i.key()).collect();
        assert_eq!(keys.len(), 12, "every issue has a key of its own");
    }

    #[test]
    fn the_control_renames_nobody_and_has_as_many_issues() {
        let state = replayed(Shape::Control, 6);
        assert_eq!(state.issues().count(), 12);
        assert!(state.renames().is_empty());
        assert_eq!(waiting(&state), 0);
        let keys: Vec<String> = state.issues().map(|issue| issue.key().to_owned()).collect();
        assert_eq!(
            keys,
            (1..=12).map(|n| format!("ex-{n}")).collect::<Vec<_>>()
        );
    }

    /// The keys of `state`'s issues, in the order replay created them.
    fn keys(state: &State) -> Vec<String> {
        state.issues().map(|issue| issue.key().to_owned()).collect()
    }

    #[test]
    fn numbered_copies_count_upward_past_every_number_taken_to_the_controls_keys() {
        let numbered = |from: usize, to: usize| -> Vec<String> {
            (from..=to).map(|n| format!("ex-1.{n}")).collect()
        };
        // the copies end up with the keys the second machine of each control creates
        for (shape, control, waits) in [
            (Shape::Numbered, Shape::NumberedControl, 0),
            (Shape::NumberedWaiting, Shape::NumberedControlWaiting, 6),
        ] {
            let state = replayed(shape, 6);
            let last = 6 + waits;
            let mut expected = numbered(1, 6);
            expected.extend(numbered(last + 1, last + 6));
            assert_eq!(keys(&state), expected, "{shape:?}");
            assert_eq!(state.renames().len(), 6, "{shape:?}");
            for (n, rename) in state.renames().iter().enumerate() {
                assert_eq!(rename.from, format!("ex-1.{}", n + 1), "{shape:?}");
                assert_eq!(rename.to, format!("ex-1.{}", last + n + 1), "{shape:?}");
            }
            assert_eq!(waiting(&state), waits, "{shape:?}");
            let state = replayed(control, 6);
            assert_eq!(keys(&state), expected, "{control:?}");
            assert!(state.renames().is_empty(), "{control:?}");
            assert_eq!(
                waiting(&state),
                waits,
                "{control:?}: nothing it waits for arrives"
            );
        }
        // with gaps, each copy takes the even number after its own, the control's keys
        let state = replayed(Shape::NumberedGaps, 6);
        let odd: Vec<String> = (1..=6).map(|n| format!("ex-1.{}", 2 * n - 1)).collect();
        let even: Vec<String> = (1..=6).map(|n| format!("ex-1.{}", 2 * n)).collect();
        assert_eq!(keys(&state), [odd.clone(), even.clone()].concat());
        assert_eq!(state.renames().len(), 6);
        let control = replayed(Shape::NumberedGapsControl, 6);
        assert_eq!(keys(&control), [odd, even].concat());
        assert!(control.renames().is_empty());
        // what the first machine's issues wait for is claimed, though no issue has it
        let state = replayed(Shape::NumberedWaiting, 6);
        assert!(state.claimed_keys().contains("ex-1.7"));
        assert!(!state.claimed_keys().contains("ex-1.19"));
    }

    #[test]
    fn issues_that_wait_for_keys_that_never_come_still_wait_in_both_shapes() {
        for (shape, renamed) in [(Shape::RenamedWaiting, 6), (Shape::ControlWaiting, 0)] {
            let state = replayed(shape, 6);
            assert_eq!(state.issues().count(), 12, "{shape:?}");
            assert_eq!(state.renames().len(), renamed, "{shape:?}");
            assert_eq!(waiting(&state), 6, "{shape:?}: the first machine's issues");
        }
    }

    #[test]
    fn a_copy_never_takes_a_key_an_import_waits_for() {
        let state = replayed(Shape::RenamedWaiting, 6);
        for rename in state.renames() {
            assert!(!rename.to.starts_with("ex-gone-"), "{rename:?}");
        }
        // the keys waited for are claimed, though no issue has them
        assert!(state.claimed_keys().contains("ex-gone-3"));
    }

    #[test]
    fn the_figures_name_every_shape_and_how_many_it_renamed() {
        let line = measure(4, Some("label")).unwrap();
        for shape in Shape::ALL {
            let name = shape.name();
            assert!(line.contains(&format!("\"{name}_seconds\"")), "{line}");
        }
        assert!(line.contains("\"renamed_renamed\": 4"), "{line}");
        assert!(line.contains("\"control_renamed\": 0"), "{line}");
        assert!(line.contains("\"renamed_waiting_waiting\": 4"), "{line}");
        assert!(line.contains("\"numbered_renamed\": 4"), "{line}");
        assert!(line.contains("\"numbered_control_renamed\": 0"), "{line}");
        assert!(line.contains("\"numbered_waiting_waiting\": 4"), "{line}");
        assert!(line.contains("\"numbered_gaps_renamed\": 4"), "{line}");
        assert!(
            line.contains("\"numbered_gaps_control_renamed\": 0"),
            "{line}"
        );
        assert!(line.contains("\"copies\": 4"), "{line}");
    }
}
