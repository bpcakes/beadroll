//! What bead prints, read as the tests read it.

/// `text` without bead's colour codes (`ESC [`, digits and semicolons, `m`); anything else after
/// an `ESC [` fails the test, naming where.
pub fn uncoloured(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find("\u{1b}[") {
        out.push_str(&rest[..at]);
        let after = &rest[at + 2..];
        let end = after
            .find(|c: char| !(c.is_ascii_digit() || c == ';'))
            .unwrap_or(after.len());
        assert!(
            after[end..].starts_with('m'),
            "not a colour code: {:?}",
            rest.chars().take(100).collect::<String>()
        );
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}
