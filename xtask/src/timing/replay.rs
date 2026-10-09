//! `timing --replay N`: how long beadroll-core takes to replay N events (a parsed journal to its
//! State), against the 20 ms Appendix A of `docs/RUST-PORT.md` estimates for 10,000 in a release
//! build; Python's replay of the same journal is measured beside it. Evidence, never a gate.

use std::io::Write as _;
use std::process::{Command, Stdio};
use std::time::Instant;

use beadroll_core::journal::{Journal, read_segment};
use beadroll_core::json::{Format, Object, Value, write};
use beadroll_core::replay::replay;

use crate::workspace::{self, Failure, json_string};

/// How many times each side replays; the fastest run counts.
const RUNS: usize = 7;

/// A history as six machines write one, every event valid: creates, edits, comments, labels,
/// dependencies, claims and closes, each machine building on what it has seen, syncing now and then.
fn history(count: usize) -> Vec<(String, Vec<u8>)> {
    let mut seed = 0x5EED_u64;
    let mut next = move |n: usize| -> usize {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        usize::try_from((seed >> 33) % u64::try_from(n.max(1)).unwrap_or(1)).unwrap_or(0)
    };
    let machines: Vec<String> = (0..6).map(|n| format!("{n:04x}machine")).collect();
    let mut heads: Vec<Vec<String>> = vec![Vec::new(); machines.len()];
    let mut lines: Vec<Vec<u8>> = vec![Vec::new(); machines.len()];
    let mut issues: Vec<String> = Vec::new();
    let mut claims: Vec<(String, String)> = Vec::new();
    for n in 0..count {
        let m = if n == 0 { 0 } else { next(machines.len()) };
        if n > 0 && next(10) == 0 {
            let other = next(machines.len());
            let union: Vec<String> = heads[m].iter().chain(&heads[other]).cloned().collect();
            heads[m] = union;
        }
        let millis = 1_791_558_245_123 + u64::try_from(n).unwrap_or(0);
        let id = format!(
            "{:08x}-{:04x}-7000-8000-{:012x}",
            millis >> 16,
            millis & 0xffff,
            n
        );
        let (op, issue, data) = if n == 0 {
            (
                "init",
                id.clone(),
                obj(&[("prefix", "ex".into()), ("name", "ex".into())]),
            )
        } else if issues.is_empty() || next(10) == 0 {
            issues.push(id.clone());
            let fields = obj(&[
                ("title", format!("issue {n}").into()),
                ("priority", Value::from(2)),
            ]);
            (
                "create",
                id.clone(),
                obj(&[("key", format!("ex-{n}").into()), ("fields", fields)]),
            )
        } else {
            let issue = issues[next(issues.len())].clone();
            let (op, data) = match next(10) {
                0..=2 => (
                    "set",
                    obj(&[("fields", obj(&[("title", format!("edit {n}").into())]))]),
                ),
                3 | 4 => ("comment", obj(&[("text", format!("note {n}").into())])),
                5 => (
                    "label_add",
                    obj(&[("labels", Value::Array(vec!["p1".into()]))]),
                ),
                6 => {
                    let to = issues[next(issues.len())].clone();
                    (
                        "dep_add",
                        obj(&[("to", to.into()), ("type", "blocks".into())]),
                    )
                }
                7 => {
                    claims.push((id.clone(), issue.clone()));
                    ("claim", Value::Object(Object::new()))
                }
                8 => match claims.iter().rev().find(|(_, on)| *on == issue) {
                    Some((claim, _)) => ("release", obj(&[("claim", claim.clone().into())])),
                    None => ("reopen", Value::Object(Object::new())),
                },
                _ => ("close", obj(&[("reason", "done".into())])),
            };
            (op, issue, data)
        };
        let deps: Vec<Value> = heads[m].iter().map(|d| Value::from(d.as_str())).collect();
        let actor = obj(&[
            ("agent", "claude".into()),
            ("thread", format!("t{m}").into()),
        ]);
        let event = obj(&[
            ("v", Value::from(1)),
            ("id", id.clone().into()),
            ("replica", machines[m].clone().into()),
            ("deps", Value::Array(deps)),
            ("actor", actor),
            ("op", op.into()),
            ("issue", issue.into()),
            ("data", data),
        ]);
        lines[m].extend(
            write(&event, Format::COMPACT)
                .unwrap_or_default()
                .into_bytes(),
        );
        lines[m].push(b'\n');
        heads[m] = vec![id];
    }
    machines.into_iter().zip(lines).collect()
}

fn obj(members: &[(&str, Value)]) -> Value {
    Value::Object(
        members
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect(),
    )
}

/// The journal of `segments`, parsed and settled: what replay starts from.
fn journal(segments: &[(String, Vec<u8>)]) -> Journal {
    let mut journal = Journal::new();
    for (replica, data) in segments {
        let origin = format!("{replica}/000001.jsonl");
        for event in read_segment(data, &origin, Some(&mut journal.invalid)) {
            journal.add(event, replica, &origin, true);
        }
    }
    journal.settle();
    journal
}

/// Python's fastest replay of the same journal, in seconds.
fn python(segments: &[(String, Vec<u8>)]) -> Result<f64, Failure> {
    const SCRIPT: &str = "\
import sys, time
sys.path.insert(0, sys.argv[1])
import beadroll as bead
segments, journal = [], bead.Journal()
for line in sys.stdin.buffer.read().split(b'\\0'):
    if line:
        replica, data = line.split(b' ', 1)
        segments.append((replica.decode(), data))
for replica, data in segments:
    origin = replica + '/000001.jsonl'
    for event in bead.read_segment(data, origin, journal.invalid):
        journal.add(event, replica, origin)
journal.settle()
best = None
for _ in range(int(sys.argv[2])):
    import copy
    trial = copy.deepcopy(journal)
    start = time.perf_counter()
    bead.replay(trial)
    took = time.perf_counter() - start
    best = took if best is None else min(best, took)
print(best)
";
    let root = workspace::root();
    let mut child = Command::new(workspace::python())
        .args(["-c", SCRIPT])
        .arg(&root)
        .arg(RUNS.min(3).to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|error| Failure::Failed(format!("cannot run python: {error}")))?;
    let mut input = Vec::new();
    for (replica, data) in segments {
        input.extend(replica.as_bytes());
        input.push(b' ');
        input.extend(data);
        input.push(0);
    }
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(&input)
            .map_err(|error| Failure::Failed(format!("cannot feed python: {error}")))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| Failure::Failed(format!("python failed: {error}")))?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.trim()
        .parse()
        .map_err(|_| Failure::Failed(format!("python answered {text:?}")))
}

/// Measures and records; returns the line it recorded.
pub(crate) fn measure(count: usize, label: Option<&str>) -> Result<String, Failure> {
    let segments = history(count);
    let parsed = journal(&segments);
    let events = parsed.events().len();
    let mut best = f64::INFINITY;
    let mut issues = 0;
    for _ in 0..RUNS {
        let trial = parsed.clone();
        let clock = Instant::now();
        let state =
            replay(trial).map_err(|error| Failure::Failed(format!("replay failed: {error}")))?;
        best = best.min(clock.elapsed().as_secs_f64());
        issues = state.issues().count();
    }
    let python = python(&segments)?;
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    let label = label.map_or_else(|| format!("replay of {count} events"), str::to_owned);
    Ok(format!(
        "{{\"label\": {}, \"events\": {events}, \"issues\": {issues}, \"profile\": {}, \"runs\": {RUNS}, \
         \"seconds\": {best:.4}, \"python_seconds\": {python:.4}, ",
        json_string(&label),
        json_string(profile),
    ))
}
