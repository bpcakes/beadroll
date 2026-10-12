//! G38: an imported record whose `comments` is not a list of objects fails the import whole with
//! VALIDATION, from `import` and from `init --import` alike.

use beadroll_core::json::{Format, Object, write};
use beadroll_test_support::{Json, json};

/// `_g38_common`: a Beads record's required fields.
fn common() -> Json {
    json!({
        "status": "open",
        "priority": 2,
        "issue_type": "task",
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-01T00:00:00Z",
        "created_by": "seed",
    })
}

/// `_g38_shapes`: `comments` values that are not a list of objects, each with its name.
fn shapes() -> [(&'static str, Json); 6] {
    [
        ("list of strings", json!(["text"])),
        ("object", json!({"id": 1})),
        ("number", json!(5)),
        ("string", json!("abc")),
        ("true", json!(true)),
        ("list holding null", json!([null])),
    ]
}

/// `_g38_tail`: the last line of `text` that is not blank, or nothing.
fn tail(text: &str) -> &str {
    let mut lines = text.trim().split('\n');
    lines.rfind(|line| !line.trim().is_empty()).unwrap_or("")
}

/// The Beads record `{"id": KEY, "title": TITLE, **_g38_common()}`, then the members of `more`.
fn record(key: &str, title: &str, more: Json) -> Json {
    let mut members = Object::new();
    members.insert("id".into(), Json::from(key));
    members.insert("title".into(), Json::from(title));
    for part in [common(), more] {
        match part {
            Json::Object(part) => members.extend(*part),
            other => panic!("{other:?} is not an object"),
        }
    }
    Json::from(members)
}

/// `record` with a `comments` member holding `comments`.
fn with_comments(key: &str, title: &str, comments: Json) -> Json {
    let mut more = Object::new();
    more.insert("comments".into(), comments);
    record(key, title, Json::from(more))
}

/// Records as spaced, ASCII-escaped JSON lines.
fn jsonl(records: &[Json]) -> String {
    records
        .iter()
        .map(|record| write(record, Format::ASCII).unwrap() + "\n")
        .collect()
}

mod g38_import_comments_of_the_wrong_shape_are_validation {
    use beadroll_test_support::{Machine, code, json};

    use super::{jsonl, record, shapes, tail, with_comments};

    /// G38: an imported record whose `comments` is not a list of objects crashes `import_records`
    /// with AttributeError/TypeError before validation runs, instead of failing whole with
    /// VALIDATION.
    #[test]
    fn wrong_comment_shapes_fail_the_import_with_validation() {
        let sandbox = crate::sandbox();
        let m = Machine::new(&sandbox, "g38");
        m.bead(["init", "--prefix", "ex"]);
        let good = record("ex-good", "good", json!({}));
        for (name, comments) in shapes() {
            let path = sandbox
                .root()
                .join(format!("{}.jsonl", name.replace(' ', "-")));
            let bad = with_comments("ex-bad", name, comments);
            std::fs::write(&path, jsonl(&[good.clone(), bad])).unwrap();
            let proc = m.run(["import", path.to_str().unwrap(), "--json"]);
            assert!(
                !proc.crashed(),
                "import with comments as {name} crashed: {}",
                tail(&proc.stderr)
            );
            assert_eq!(proc.returncode(), 1, "{name}: {}", proc.stdout);
            assert_eq!(code(&proc), "VALIDATION", "{name}");
        }
        // failed whole: not even ex-good
        let listed = m.bead(["list", "--all", "--limit", "0"]).unwrap();
        assert_eq!(listed["total"].as_i64(), Some(0), "{listed:?}");
        let ok = sandbox.root().join("ok.jsonl");
        let comment = json!({
            "id": 7,
            "author": "a",
            "text": "kept",
            "created_at": "2026-01-01T00:00:00Z",
        });
        let kept = with_comments("ex-good", "good", json!([comment]));
        std::fs::write(&ok, jsonl(&[kept])).unwrap();
        let imported = m.bead(["import", ok.to_str().unwrap()]).unwrap();
        assert_eq!(imported["imported"].as_i64(), Some(1), "{imported:?}");
        let shown = m.show("ex-good");
        let comments = shown["comments"].as_array().expect("the issue's comments");
        let texts: Vec<&str> = comments
            .iter()
            .map(|comment| comment["text"].as_str().expect("a comment's text"))
            .collect();
        assert_eq!(texts, ["kept"]);
    }

    /// G38: `init --import` of such a record also tracebacks instead of a VALIDATION error.
    #[test]
    fn init_import_with_wrong_comment_shape_is_validation() {
        let sandbox = crate::sandbox();
        let path = sandbox.root().join("init.jsonl");
        let bad = with_comments("bx-bad", "t", json!(["x"]));
        std::fs::write(&path, jsonl(&[bad])).unwrap();
        let m = Machine::new(&sandbox, "g38init");
        let path = path.to_str().unwrap();
        let proc = m.run(["init", "--prefix", "bx", "--import", path, "--json"]);
        assert!(
            !proc.crashed(),
            "init --import with comments as a list of strings crashed: {}",
            tail(&proc.stderr)
        );
        assert_eq!(code(&proc), "VALIDATION");
    }
}
