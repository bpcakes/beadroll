//! G31: an export whose text holds U+2028, U+2029 or U+0085 imports again, and a Beads file with
//! CRLF line ends and blank lines between its records is read whole.

/// `_g31_titles`: titles holding U+2028, U+2029 and U+0085 (escapes only: raw ones would be split
/// by line-based tools).
fn titles() -> [&'static str; 3] {
    ["line\u{2028}sep", "para\u{2029}sep", "nel\u{0085}x"]
}

/// `_g31_tail`: the last line of `text` that is not blank, or nothing.
fn tail(text: &str) -> &str {
    let mut lines = text.trim().split('\n');
    lines.rfind(|line| !line.trim().is_empty()).unwrap_or("")
}

mod g31_export_with_unicode_line_separators_imports_again {
    use beadroll_core::json::{Format, write};
    use beadroll_test_support::{Json, Machine, json, parse_json};

    use super::{tail, titles};

    /// The id of the issue a `--json` result is: `result["id"]`.
    fn id(result: Option<Json>) -> String {
        let result = result.expect("bead printed a result");
        result["id"].as_str().expect("an id").to_owned()
    }

    /// G31: `export` writes U+2028/U+2029/U+0085 raw inside JSON strings (valid JSONL), but `import`
    /// splits the file at Unicode line boundaries, so bead cannot import its own export.
    #[test]
    fn own_export_with_unicode_line_separators_imports_again() {
        let sandbox = crate::sandbox();
        let src = Machine::new(&sandbox, "g31src");
        src.bead(["init", "--prefix", "ex"]);
        let mut keys = Vec::new();
        for title in titles() {
            let description = format!("d:{title}");
            let key = id(src.bead(["create", title, "-d", &description]));
            src.bead(["comments", "add", &key, &format!("c:{title}")]);
            keys.push((title, key));
        }
        let out = sandbox.root().join("export.jsonl");
        let out = out.to_str().expect("a UTF-8 path");
        let proc = src.run(["export", "-o", out, "--json"]);
        assert_eq!(proc.returncode(), 0, "{}", proc.stderr);
        let raw = std::fs::read(out).unwrap();
        // three records, three physical lines
        assert_eq!(raw.iter().filter(|byte| **byte == b'\n').count(), 3);
        // written raw, which is valid JSON
        let separator = "\u{2028}".as_bytes();
        let written_raw = raw.windows(separator.len()).any(|w| w == separator);
        assert!(written_raw, "{}", String::from_utf8_lossy(&raw));
        let dst = Machine::new(&sandbox, "g31dst");
        // its own tracker: not a second machine of the source's
        dst.bead(["init", "--prefix", "zz", "--offline"]);
        let proc = dst.run(["import", out, "--offline", "--json"]);
        assert_eq!(
            proc.returncode(),
            0,
            "bead could not import its own export: {}",
            tail(&proc.stderr)
        );
        // (--offline: the result also counts the unpublished)
        let imported = parse_json(&proc.stdout).unwrap();
        assert_eq!(imported["imported"].as_i64(), Some(3), "{}", proc.stdout);
        for (title, key) in &keys {
            let issue = dst.show(key);
            assert_eq!(issue["title"].as_str(), Some(*title));
            let description = format!("d:{title}");
            assert_eq!(issue["description"].as_str(), Some(description.as_str()));
            let comments = issue["comments"].as_array().expect("the issue's comments");
            let texts: Vec<&str> = comments
                .iter()
                .map(|comment| comment["text"].as_str().expect("a comment's text"))
                .collect();
            assert_eq!(texts, [format!("c:{title}")]);
        }
    }

    /// G31: `init --import` of a br-written record whose title holds a raw U+2028 fails as invalid
    /// JSONL. CRLF line ends and blank lines must keep working.
    #[test]
    fn init_import_of_a_br_export_with_a_raw_line_separator() {
        let sandbox = crate::sandbox();
        let record = json!({
            "id": "bx-o5n",
            "title": "line\u{2028}sep",
            "status": "open",
            "priority": 2,
            "issue_type": "task",
            "created_at": "2026-10-07T18:24:00Z",
            "updated_at": "2026-10-07T18:24:00Z",
        });
        // {**record, "id": "bx-crlf", "title": "plain"}: the same members in the same places
        let mut other = record.clone();
        let Json::Object(members) = &mut other else {
            panic!("a record is an object")
        };
        members.insert("id".into(), Json::from("bx-crlf"));
        members.insert("title".into(), Json::from("plain"));
        let path = sandbox.root().join("issues.jsonl");
        // Write the first record with raw Unicode and the second with ASCII escapes.
        let mut bytes = write(&record, Format::SPACED).unwrap().into_bytes();
        bytes.extend_from_slice(b"\r\n\n");
        bytes.extend(write(&other, Format::ASCII).unwrap().into_bytes());
        bytes.extend_from_slice(b"\r\n");
        std::fs::write(&path, bytes).unwrap();
        let machine = Machine::new(&sandbox, "g31br");
        let path = path.to_str().expect("a UTF-8 path");
        let proc = machine.run(["init", "--prefix", "bx", "--import", path, "--json"]);
        assert_eq!(
            proc.returncode(),
            0,
            "init --import refused a valid br export: {}",
            tail(&proc.stderr)
        );
        let imported = parse_json(&proc.stdout).unwrap();
        assert_eq!(imported["imported"].as_i64(), Some(2), "{}", proc.stdout);
        let shown = machine.show("bx-o5n");
        assert_eq!(shown["title"].as_str(), Some("line\u{2028}sep"));
    }
}
