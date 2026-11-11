//! Fixed comment values keep member reads and whole-object rendering independently checked.

use std::sync::Arc;

use crate::id::EventId;
use crate::json::{self, Object, Value};

use super::Comment;

fn native(seq: u64, where_: Option<Value>) -> Comment {
    Comment::Native {
        uid: EventId::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0001),
        seq,
        author: Arc::from("codex:t"),
        text: "said".to_owned(),
        where_: where_.map(Arc::new),
    }
}

/// Neither expected values nor their order come from either method under test.
fn assert_members(comment: Comment, expected: &str) {
    let expected = json::parse(expected.as_bytes()).unwrap().value;
    let expected = expected.as_object().unwrap();
    for name in [
        "uid",
        "seq",
        "author",
        "text",
        "created_at",
        "where",
        "id",
        "extra",
        "other",
    ] {
        assert_eq!(comment.get(name), expected.get(name).cloned(), "{name}");
    }
    let whole = comment.to_value();
    assert_eq!(&whole, expected);
    assert_eq!(
        whole.keys().collect::<Vec<_>>(),
        expected.keys().collect::<Vec<_>>()
    );
}

/// AS04: native member values, absent fields and order stay fixed, with and without a place.
#[test]
fn native_comment_members_match_fixed_values() {
    assert_members(
        native(3, None),
        r#"{"uid":"01900000-0000-7000-8000-000000000001","seq":3,"author":"codex:t","text":"said","created_at":"2024-06-10T02:35:18.400Z"}"#,
    );
    let mut place = Object::new();
    place.insert("branch".into(), Value::from("main"));
    assert_members(
        native(3, Some(Value::from(place))),
        r#"{"uid":"01900000-0000-7000-8000-000000000001","seq":3,"author":"codex:t","text":"said","created_at":"2024-06-10T02:35:18.400Z","where":{"branch":"main"}}"#,
    );
}

/// AS04: the optimized member reader keeps the whole object's existing sequence saturation.
#[test]
fn native_comment_sequence_keeps_legacy_saturation() {
    assert_members(
        native(u64::MAX, None),
        r#"{"uid":"01900000-0000-7000-8000-000000000001","seq":9223372036854775807,"author":"codex:t","text":"said","created_at":"2024-06-10T02:35:18.400Z"}"#,
    );
}

/// AS04: imported members keep their own values and order, including null and unknown members.
#[test]
fn imported_comment_members_match_fixed_values() {
    let mut imported = Object::new();
    imported.insert("id".into(), Value::from("c-1"));
    imported.insert("uid".into(), Value::Null);
    imported.insert("text".into(), Value::from("brought along"));
    imported.insert("extra".into(), Value::Bool(true));
    assert_members(
        Comment::Imported(imported),
        r#"{"id":"c-1","uid":null,"text":"brought along","extra":true}"#,
    );
}
