//! U06 (`docs/FINDINGS.md`): a scheme is ASCII.

use std::ffi::OsStr;
use std::path::Path;

use beadroll_git::{normalize_remote, without_password};

fn wp(url: &str) -> (String, bool) {
    let (out, dropped) = without_password(OsStr::new(url));
    (out.into_string().unwrap(), dropped)
}

/// Case-insensitive `[a-z]` also matched ſ (U+017F) and K (U+212A), so `httpſ://host/x` derived the
/// workspace id of `https://host/x`. A scheme is ASCII, as git reads one; with any other letter the
/// string has no scheme at all.
#[test]
fn a_scheme_is_ascii_whatever_case_folding_would_allow() {
    let norm = |u: &str| normalize_remote(OsStr::new(u), Path::new("/"));
    assert_eq!(norm("HTTPS://Host/X.git"), "host/x"); // ASCII case still folds
    assert_eq!(
        wp("HTTPS://user:secret@host/x"),
        ("HTTPS://host/x".into(), true)
    );
    assert_eq!(norm("http\u{17f}://host/x"), "http\u{17f}://host/x");
    for url in ["http\u{17f}://user@host/x", "\u{212a}eep://user@host/x"] {
        assert_eq!(wp(url), (url.into(), false), "{url}");
    }
}
