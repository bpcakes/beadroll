//! G64: writes on a broken own chain are refused with a structured error and write nothing.

mod g64_writes_on_a_broken_own_chain {
    use std::path::{Path, PathBuf};

    use beadroll_core::json::{Format, Style, write};
    use beadroll_test_support::{Json, Machine, code, parse_json};

    /// Compact, ASCII-escaped JSON.
    const COMPACT_ASCII: Format = Format {
        style: Style::Compact,
        ascii: true,
        sorted: false,
    };

    /// The segments of a journal, `*.jsonl` in `dir`, by name: `sorted(dir.glob("*.jsonl"))`.
    fn segments(dir: &Path) -> Vec<PathBuf> {
        let entries = std::fs::read_dir(dir).expect("a journal's folder");
        let mut found: Vec<PathBuf> = entries
            .map(|entry| entry.expect("a directory entry").path())
            .filter(|path| {
                let name = path.file_name().expect("a name");
                name.as_encoded_bytes().ends_with(b".jsonl")
            })
            .collect();
        found.sort();
        found
    }

    /// The last `count` Unicode scalar values of `text`.
    fn tail(text: &str, count: usize) -> String {
        let skip = text.chars().count().saturating_sub(count);
        text.chars().skip(skip).collect()
    }

    /// `event` made the first of a chain of its own, as a copied identity or a write made while the
    /// journal directory could not be listed leaves one: its id (and the issue it creates) the
    /// same but for the last 12 digits, no causes, and the key `cb-forked`.
    fn forked(event: &mut Json) {
        let Json::Object(members) = event else {
            panic!("an event is an object: {event:?}");
        };
        let id = members["id"].as_str().expect("an event's id");
        let id = format!("{}0123456789ab", &id[..id.len().saturating_sub(12)]);
        members.insert("id".into(), Json::from(id.as_str()));
        members.insert("issue".into(), Json::from(id));
        members.insert("deps".into(), Json::Array(Vec::new()));
        let Some(Json::Object(data)) = members.get_mut("data") else {
            panic!("a create event has data");
        };
        data.insert("key".into(), Json::from("cb-forked"));
    }

    /// G64: once this machine's own journal has an event that does not build on its predecessor
    /// (a copied identity, or a write made while the journal directory could not be listed),
    /// every later local write would be inert on replay. `create` then answers `JOURNAL_FORK`
    /// and appends nothing (it crashed with a KeyError at `cmd_create`'s
    /// `tx.state.issues[uid]`), and `update` does not exit 0 having changed nothing (other writes,
    /// update, comments and close, exited 0, appended an event and changed nothing).
    #[test]
    fn create_on_a_broken_own_chain_is_a_structured_error_and_writes_nothing() {
        let sandbox = crate::sandbox();
        let a = Machine::new(&sandbox, "a");
        a.bead(["init", "--prefix", "cb"]);
        let created = a.bead(["create", "before the break"]).expect("a result");
        let key = created["id"].as_str().expect("an id");
        let info = a.bead(["info"]).expect("bead info prints a record");
        let own = info.get("machine").expect("info's machine");
        let own = own.as_str().expect("a machine id");
        let segment = segments(&a.store().join("events").join(own))
            .into_iter()
            .next()
            .expect("a segment of the machine's own journal");
        let text = std::fs::read_to_string(&segment).unwrap();
        // The events are ASCII and have no line separator other than newline.
        let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
        let mut bad = parse_json(lines.last().expect("an event")).unwrap();
        forked(&mut bad);
        lines.push(write(&bad, COMPACT_ASCII).unwrap());
        std::fs::write(&segment, lines.join("\n") + "\n").unwrap();
        let before = std::fs::read_to_string(&segment).unwrap();
        let proc = a.run(["create", "after the break", "--json"]);
        assert!(
            !proc.crashed(),
            "create on a broken own chain crashed:\n{}",
            tail(&proc.stderr, 400)
        );
        assert_eq!(proc.returncode(), 1, "{}", proc.stdout);
        assert_eq!(code(&proc), "JOURNAL_FORK");
        assert_eq!(
            std::fs::read_to_string(&segment).unwrap(),
            before,
            "an inert event was appended"
        );
        let title = a.run(["update", key, "--title", "renamed", "--json"]);
        let shown = a.show(key);
        let shown = shown.get("title").expect("show's title");
        assert_ne!(
            (title.returncode(), shown),
            (0, &Json::from("before the break")),
            "update exited 0 and changed nothing"
        );
    }
}
