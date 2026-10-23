//! Metadata's typed path and legacy fallback preserve both reads and whole-file rewrites.

use super::*;
use crate::json::{Format, write};

#[test]
fn metadata_preserves_values_order_and_duplicate_members() {
    for text in [
        r#"{"name":"n","published":{"a":1,"b":18446744073709551615,"a":-2},"remote":"r","x":[1]}"#,
        r#"{"published":{"a":1.9,"b":" 2 ","c":false,"d":{},"e":[1]},"x":null}"#,
        r#"{"published":[],"x":1,"published":{"a":3},"x":2}"#,
        r#"{"published":{"a":1},"published":false}"#,
        r#"{"published":{"a":null,"a":2},"remote":false}"#,
    ] {
        let expected = json::parse(text.as_bytes()).unwrap().value;
        let meta = Metadata::read(text.as_bytes()).unwrap().unwrap();
        assert_eq!(
            write(&Value::from(meta.into_object()), Format::COMPACT).unwrap(),
            write(&expected, Format::COMPACT).unwrap(),
            "{text}"
        );
    }
}

#[test]
fn published_counts_keep_legacy_coercions_and_membership() {
    let meta = Metadata::read(br#"{"published":{"int":-2,"float":-1.9,"text":" 3 ","yes":true,"no":false,"null":null,"array":[],"object":{},"bad":[1],"empty":""}}"#).unwrap().unwrap();
    let counts = meta.published().unwrap();
    for (name, n) in [
        ("int", -2),
        ("float", -1),
        ("text", 3),
        ("yes", 1),
        ("no", 0),
        ("null", 0),
        ("array", 0),
        ("object", 0),
        ("empty", 0),
        ("absent", 0),
    ] {
        assert_eq!(counts.count(name), Some(Int::from(n)), "{name}");
    }
    assert_eq!(counts.count("bad"), None);
    assert!(counts.contains_key("bad"));
    assert!(!counts.contains_key("absent"));
}

#[test]
fn malformed_metadata_keeps_the_json_diagnostic() {
    for text in ["{", "{} trailing", r#"{"published":{"a":1e400}}"#] {
        assert_eq!(
            Metadata::read(text.as_bytes()).unwrap_err(),
            json::parse(text.as_bytes()).unwrap_err()
        );
    }
    assert!(Metadata::read(b"[]").unwrap().is_none());
}
