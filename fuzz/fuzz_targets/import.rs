//! The Beads importer (roll-k2wy.14.4). Any text, imported into a small tracker that already holds
//! issues, one of them with dependencies that wait for keys: an import refused is refused with an
//! error, never a panic, and the dry run finds a problem exactly when the import is refused
//! (`dry_run::problems`). An import that goes through plans events `Tx::emit` writes (each valid,
//! nested within the limit, written), its preview counts what it plans, and replayed after the
//! tracker's they keep every new record's id as its key. That tracker's export is read back by the
//! importer's reader, and imported again it plans nothing.
#![no_main]

use std::sync::LazyLock;

use beadroll_core::event::{MAX_NESTING, event_problem, payload_problem, write_problem};
use beadroll_core::import::dry_run::{preview, problems};
use beadroll_core::import::{Step, Target, plan_import, read_records};
use beadroll_core::journal::Journal;
use beadroll_core::json::{self, Format, Object, Value};
use beadroll_core::replay::replay;
use beadroll_core::state::State;
use beadroll_fuzz::{event, id};
use libfuzzer_sys::fuzz_target;

const FILE: &str = "issues.jsonl";

/// The tracker's events: `fz-1`, which waits for `fz-9` and `gone`, and `fz-2`.
static TRACKER: LazyLock<Vec<Value>> = LazyLock::new(|| {
    let members = |pairs: Vec<(&str, Value)>| -> Object {
        pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect()
    };
    let fields = |title: &str| {
        Value::from(members(vec![
            ("title", Value::from(title)),
            ("status", Value::from("open")),
            ("priority", Value::from(2)),
            ("issue_type", Value::from("task")),
        ]))
    };
    let waits = |key: &str| {
        Value::from(members(vec![
            ("depends_on_id", Value::from(key)),
            ("type", Value::from("blocks")),
        ]))
    };
    let init = members(vec![("prefix", Value::from("fz"))]);
    let one = members(vec![
        ("key", Value::from("fz-1")),
        ("fields", fields("one")),
        ("imported", Value::Bool(true)),
        ("dangling", Value::Array(vec![waits("fz-9"), waits("gone")])),
    ]);
    let two = members(vec![
        ("key", Value::from("fz-2")),
        ("fields", fields("two")),
    ]);
    vec![
        event(&id(1), "init", None, &[], init),
        event(&id(2), "create", None, &[id(1)], one),
        event(&id(3), "create", None, &[id(2)], two),
    ]
});

fn tracker(events: &[Value]) -> State {
    let mut journal = Journal::new();
    for event in events {
        journal.add(
            event.clone(),
            "0000aa01",
            "events/0000aa01/000001.jsonl",
            true,
        );
    }
    replay(journal).expect("the tracker replays")
}

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return; // bead reads an export as UTF-8 or refuses it (`read_utf8`)
    };
    let state = tracker(&TRACKER);
    let found = problems(&state, text, FILE);
    let planned =
        read_records(text, FILE).and_then(|records| Ok((plan_import(&state, &records)?, records)));
    let (plan, records) = match planned {
        Err(refused) => {
            assert!(
                !found.is_empty(),
                "refused ({refused}), but the dry run found nothing"
            );
            return;
        }
        Ok(planned) => planned,
    };
    assert!(
        found.is_empty(),
        "imported, but the dry run found {found:?}"
    );
    let would = preview(&state, &records);
    assert_eq!(would.records, records.len());
    assert_eq!(would.known + plan.imported, records.len(), "{would:?}");

    // The plan's events, emitted as `import_records` emits them, each after the last.
    let mut events = TRACKER.clone();
    let mut created: Vec<String> = Vec::new();
    for step in plan.steps {
        let at = id(events.len() as u64 + 1);
        let last = [id(events.len() as u64)];
        let emitted = match step {
            Step::DepRemove { issue, data } => {
                event(&at, "dep_remove", Some(&issue.to_string()), &last, data)
            }
            Step::Create(data) => {
                created.push(at.clone());
                event(&at, "create", None, &last, data)
            }
            Step::DepAdd { issue, to, data } => {
                let to = match to {
                    Target::Issue(uid) => uid.to_string(),
                    Target::Created(n) => created[n].clone(),
                };
                let mut payload = Object::new();
                payload.insert("to".into(), Value::from(to));
                payload.extend(data);
                event(&at, "dep_add", Some(&created[issue]), &last, payload)
            }
        };
        let problem = event_problem(&emitted)
            .or_else(|| payload_problem(&emitted))
            .or_else(|| write_problem(&emitted));
        assert_eq!(problem, None, "an event planned is refused: {emitted:?}");
        assert!(
            !emitted.deeper_than(MAX_NESTING),
            "an event planned nests too deep"
        );
        json::write(&emitted, Format::COMPACT).expect("an event planned is written");
        events.push(emitted);
    }
    let imported = tracker(&events);
    let keys = imported.keys();
    for record in &records {
        let key = record.get("id").and_then(Value::as_str).unwrap_or_default();
        assert!(
            keys.contains_key(key),
            "record {key:?} has no issue of its key"
        );
    }

    let mut export = String::new();
    for record in imported.export_records() {
        let line = json::write(&Value::from(record), Format::COMPACT).expect("export is written");
        export.push_str(&line);
        export.push('\n');
    }
    let exported = match read_records(&export, "export.jsonl") {
        Ok(exported) => exported,
        Err(error) => panic!("bead's own export is refused: {error}"),
    };
    let again = plan_import(&imported, &exported).expect("bead's own export imports");
    assert!(
        again.steps.is_empty(),
        "an export imported again plans {:?}",
        again.steps
    );
});
