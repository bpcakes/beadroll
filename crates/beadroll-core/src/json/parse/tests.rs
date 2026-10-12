//! Fixed JSON parsing expectations, including error messages and positions, captured from the
//! CPython 3.14 reference (`docs/RUST-PORT.md` §5).

use super::*;
use crate::json::{Format, write};

fn message(text: &[u8]) -> String {
    parse(text)
        .map(|_| String::from("parsed"))
        .unwrap_or_else(|error| error.to_string())
}

#[test]
fn syntax_errors_read_as_cpython_reports_them() {
    let cases: [(&[u8], &str); 14] = [
        (
            b"\"\\u001F",
            "Unterminated string starting at: line 1 column 1 (char 0)",
        ),
        (
            b"\"\\u001",
            "Invalid \\uXXXX escape: line 1 column 3 (char 2)",
        ),
        (
            b"{\"a\":1,}",
            "Illegal trailing comma before end of object: line 1 column 7 (char 6)",
        ),
        (
            b"[1,]",
            "Illegal trailing comma before end of array: line 1 column 3 (char 2)",
        ),
        (
            b"[1 2]",
            "Expecting ',' delimiter: line 1 column 4 (char 3)",
        ),
        (
            b"{\"a\" 1}",
            "Expecting ':' delimiter: line 1 column 6 (char 5)",
        ),
        (b"nul", "Expecting value: line 1 column 1 (char 0)"),
        (
            b"\"\t\"",
            "Invalid control character at: line 1 column 2 (char 1)",
        ),
        (b"\"\\x\"", "Invalid \\escape: line 1 column 2 (char 1)"),
        (b"", "Expecting value: line 1 column 1 (char 0)"),
        (b"[] x", "Extra data: line 1 column 4 (char 3)"),
        (
            b"\xef\xbb\xbf[]",
            "Unexpected UTF-8 BOM (decode using utf-8-sig): line 1 column 1 (char 0)",
        ),
        (
            b"{\"a\":\"\\ud800\\udc00x",
            "Unterminated string starting at: line 1 column 6 (char 5)",
        ),
        (b"\n\n  ]", "Expecting value: line 3 column 3 (char 4)"),
    ];
    for (text, expected) in cases {
        assert_eq!(
            message(text),
            expected,
            "{:?}",
            String::from_utf8_lossy(text)
        );
    }
}

#[test]
fn numbers_json_has_no_room_for_are_refused() {
    assert_eq!(message(b"NaN"), "NaN is not a JSON number");
    assert_eq!(message(b"[-Infinity]"), "-Infinity is not a JSON number");
    assert_eq!(message(b"1e400"), "1e400 is not a JSON number");
    assert_eq!(parse(b"-0").unwrap().value, Value::from(0));
    let digits = |n: usize| "7".repeat(n);
    assert!(parse(format!("-{}", digits(MAX_DIGITS)).as_bytes()).is_ok());
    assert_eq!(
        message(format!("-{}", digits(MAX_DIGITS + 1)).as_bytes()),
        "Exceeds the limit (4300 digits) for integer string conversion: value has 4301 digits; \
         use sys.set_int_max_str_digits() to increase the limit"
    );
}

#[test]
fn nesting_stops_at_the_ceiling_and_strings_hold_any_bracket() {
    let deep = |n: usize| format!("{}{}", "[".repeat(n), "]".repeat(n));
    assert!(parse(deep(MAX_DEPTH).as_bytes()).is_ok());
    assert_eq!(
        message(deep(MAX_DEPTH + 1).as_bytes()),
        "nested deeper than 512 levels"
    );
    assert!(parse(format!("[\"{}\"]", "[{".repeat(600)).as_bytes()).is_ok());
}

#[test]
fn lines_are_utf8_and_lone_surrogates_read_as_replacement_characters() {
    assert_eq!(
        parse(b"[\x001\x00]\x00"),
        Err(ParseError::Syntax {
            message: "Expecting value",
            line: 1,
            column: 2,
            position: 1
        })
    );
    assert_eq!(parse(b"\"\xff\""), Err(ParseError::Utf8));
    for text in [b"\"a\\ud800b\"".as_slice(), b"\"a\xed\xa0\x80b\""] {
        let parsed = parse(text).unwrap();
        assert_eq!(
            (parsed.value, parsed.lone_surrogates),
            (Value::from("a\u{fffd}b"), true)
        );
    }
    let pair = parse(b"\"\\ud83d\\ude00\"").unwrap();
    assert_eq!(
        (pair.value, pair.lone_surrogates),
        (Value::from("\u{1f600}"), false)
    );
    // Two keys that read alike once each surrogate is U+FFFD become one, the last value kept.
    let merged = parse(b"{\"\\ud800\":1,\"\\udbff\":2,\"k\":3}").unwrap();
    assert_eq!(
        write(&merged.value, Format::COMPACT).unwrap(),
        "{\"\u{fffd}\":2,\"k\":3}"
    );
    // A surrogate in a value a repeated key replaced is not reported.
    let replaced = parse(b"{\"a\":\"\\ud800\",\"a\":\"x\"}").unwrap();
    assert_eq!(
        (replaced.value.get("a"), replaced.lone_surrogates),
        (Some(&Value::from("x")), false)
    );
}
