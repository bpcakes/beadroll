//! The JSON parser (roll-k2wy.14.4). Any bytes: what `json::parse` refuses is refused with an
//! error, never a panic; the value it reads nests no deeper than `MAX_DEPTH`, and written in every
//! dialect it reads back as the same value (its keys sorted, for a dialect that sorts them) and
//! writes again as the same text. A text with lone surrogate escapes is rewritten as the store
//! rewrites its small files (`Surrogates`): it reads back as the text did, and a second rewrite
//! keeps every lone escape the first one wrote.
#![no_main]

use beadroll_core::json::{self, Format, MAX_DEPTH, Surrogates, lone_surrogate_escapes};
use beadroll_fuzz::{FORMATS, identical, sorted};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(parsed) = json::parse(data) else {
        return;
    };
    assert!(
        !parsed.value.deeper_than(MAX_DEPTH),
        "read a value deeper than {MAX_DEPTH}"
    );
    for format in FORMATS {
        let text = json::write(&parsed.value, format).expect("a value read is written");
        let again = json::parse(text.as_bytes()).expect("what bead writes, it reads");
        assert!(!again.lone_surrogates, "{format:?} wrote a lone surrogate");
        let expected = if format.sorted {
            sorted(&parsed.value)
        } else {
            parsed.value.clone()
        };
        assert!(
            identical(&again.value, &expected),
            "{format:?} read back otherwise"
        );
        let rewritten = json::write(&again.value, format).expect("a value read is written");
        assert_eq!(rewritten, text, "{format:?} wrote it again otherwise");
    }
    if let (true, Ok(text)) = (parsed.lone_surrogates, std::str::from_utf8(data)) {
        let Some(first) = rewrite(text) else {
            return; // the private-use area has too few characters left to stand in
        };
        let read = |text: &str| json::parse(text.as_bytes()).expect("a rewrite reads").value;
        assert!(
            identical(&read(&first), &parsed.value),
            "rewritten otherwise"
        );
        let units = |text: &str| -> Vec<u16> {
            (lone_surrogate_escapes(text).into_iter())
                .map(|(_, unit)| unit)
                .collect()
        };
        let second = rewrite(&first).expect("a rewrite rewrites");
        assert_eq!(units(&second), units(&first), "lone surrogates lost");
    }
});

/// `text` rewritten as the store rewrites a small file that holds lone surrogates: stand-ins read
/// for them, the value written indented and ASCII-escaped, the escapes put back.
fn rewrite(text: &str) -> Option<String> {
    let (stood, surrogates) = Surrogates::stand_in(text, &[], "")?;
    let read = json::parse(stood.as_bytes()).expect("a text with stand-ins reads");
    assert!(!read.lone_surrogates, "a stand-in left a lone surrogate");
    let written = json::write(&read.value, Format::INDENTED).expect("a value read is written");
    Some(surrogates.restore(&written))
}
