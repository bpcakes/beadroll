//! Remote URL rules over OS bytes. ASCII patterns identify schemes and other structural
//! components. Trimming and lowercase mapping operate on Unicode text while retaining invalid
//! UTF-8 bytes unchanged (U06 in `docs/FINDINGS.md`).

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
use std::path::Path;

use crate::path::{exists, realpath};

/// `url` without any password in its user-info part, and whether one was there. bead never stores
/// or uses an embedded password: git's credential helpers are the place for it. Over http(s) the
/// whole user-info goes, because the user part may be the token (`TOKEN@`, `TOKEN:x-oauth-basic@`);
/// elsewhere a user stays (`ssh://user@host` names a login, not a secret).
pub fn without_password(url: &OsStr) -> (OsString, bool) {
    let bytes = url.as_bytes();
    let unchanged = || (url.to_owned(), false);
    // git's `<transport>::<address>` form hands the address, password included, to the remote helper.
    let Some((prefix_end, scheme)) = scheme_prefix(bytes) else {
        return unchanged();
    };
    let (prefix, rest) = bytes.split_at(prefix_end);
    // As git reads a URL, user-info is only in the authority.
    let end = rest
        .iter()
        .position(|b| matches!(b, b'/' | b'?' | b'#'))
        .unwrap_or(rest.len());
    let (head, after) = rest.split_at(end);
    let (authority, tail): (&[u8], Vec<u8>) = if let Some(at) = rposition(head, b'@') {
        let (authority, host) = (&head[..at], &head[at + 1..]);
        (authority, [host, after].concat())
    } else if after.contains(&b'@') && !host_and_port(head) {
        // No host[:port] git could use: `user:pass/word@host`.
        let at = rest.iter().position(|b| *b == b'@').unwrap_or_default();
        (&rest[..at], rest[at + 1..].to_vec())
    } else {
        return unchanged(); // an '@' in the path, query or fragment is not user-info
    };
    let scheme = scheme.to_ascii_lowercase();
    if scheme == b"http" || scheme == b"https" {
        return (OsString::from_vec([prefix, &tail].concat()), true);
    }
    if let Some(colon) = authority.iter().position(|b| *b == b':') {
        let user = &authority[..colon];
        let kept = if user.is_empty() {
            [prefix, &tail].concat()
        } else {
            [prefix, user, b"@", &tail].concat()
        };
        return (OsString::from_vec(kept), true);
    }
    unchanged()
}

/// `^((?:[a-z][a-z0-9+.-]*::)?([a-z][a-z0-9+.-]*)://)`, case-insensitive and ASCII: where the prefix
/// ends, and the scheme (the part before `://`).
fn scheme_prefix(bytes: &[u8]) -> Option<(usize, &[u8])> {
    let word = |from: usize| -> usize {
        let Some(first) = bytes.get(from) else {
            return from;
        };
        if !first.is_ascii_alphabetic() {
            return from;
        }
        from + 1
            + bytes[from + 1..]
                .iter()
                .take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'.' | b'-'))
                .count()
    };
    let scheme_at = |from: usize| -> Option<(usize, &[u8])> {
        let end = word(from);
        (end > from && bytes[end..].starts_with(b"://")).then(|| (end + 3, &bytes[from..end]))
    };
    let transport = word(0);
    if transport > 0
        && bytes[transport..].starts_with(b"::")
        && let Some(found) = scheme_at(transport + 2)
    {
        return Some(found);
    }
    scheme_at(0)
}

/// `(\[[^\]]*\]|[^:]*)(:[0-9]*)?`, matched whole: a host and an optional numeric port.
fn host_and_port(text: &[u8]) -> bool {
    let port = |rest: &[u8]| {
        rest.is_empty() || (rest.first() == Some(&b':') && rest[1..].iter().all(u8::is_ascii_digit))
    };
    if text.first() == Some(&b'[')
        && let Some(close) = text.iter().position(|b| *b == b']')
        && port(&text[close + 1..])
    {
        return true;
    }
    match text.iter().position(|b| *b == b':') {
        None => true,
        Some(colon) => port(&text[colon..]),
    }
}

fn rposition(bytes: &[u8], wanted: u8) -> Option<usize> {
    bytes.iter().rposition(|b| *b == wanted)
}

/// Normalises equivalent remote spellings: SSH, scp-like and HTTPS forms, .git suffixes, trailing
/// slashes and host case. Existing local paths resolve relative to cwd. Trimming and lowercase
/// mapping use Unicode characters.
pub fn normalize_remote(url: &OsStr, cwd: &Path) -> OsString {
    let u = strip_text_bytes(url.as_bytes());
    if exists(OsStr::from_bytes(u), cwd) {
        let real = realpath(OsStr::from_bytes(u), cwd);
        return OsString::from_vec(lowercase_text_bytes(&without_dot_git(
            real.as_os_str().as_bytes(),
        )));
    }
    let mut u: Vec<u8> = u.to_vec();
    // An ASCII scheme, as in without_password.
    if let Some(end) = plain_scheme(&u) {
        u.drain(..end);
    }
    // ^[^@/]+@
    if let Some(at) = u.iter().position(|b| matches!(b, b'@' | b'/'))
        && at > 0
        && u[at] == b'@'
    {
        u.drain(..=at);
    }
    // ^[^/:]+:[^/0-9] : scp-like `host:path`, made `host/path`.
    if let Some(colon) = u.iter().position(|b| matches!(b, b'/' | b':'))
        && colon > 0
        && u[colon] == b':'
        && u.get(colon + 1)
            .is_some_and(|b| *b != b'/' && !b.is_ascii_digit())
    {
        u[colon] = b'/';
    }
    let mut out = without_dot_git(&u);
    while out.last() == Some(&b'/') {
        out.pop();
    }
    OsString::from_vec(lowercase_text_bytes(&out))
}

/// `^[a-z][a-z0-9+.-]*://`, case-insensitive and ASCII: where it ends.
fn plain_scheme(bytes: &[u8]) -> Option<usize> {
    if !bytes.first()?.is_ascii_alphabetic() {
        return None;
    }
    let end = 1 + bytes[1..]
        .iter()
        .take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'.' | b'-'))
        .count();
    bytes[end..].starts_with(b"://").then_some(end + 3)
}

/// `re.sub(r"\.git/?$", "", text)`: `.git` or `.git/` at the end, or just before a final newline.
fn without_dot_git(text: &[u8]) -> Vec<u8> {
    let (body, newline) = match text.strip_suffix(b"\n") {
        Some(body) => (body, &b"\n"[..]),
        None => (text, &b""[..]),
    };
    let body = body
        .strip_suffix(b".git/")
        .or_else(|| body.strip_suffix(b".git"))
        .unwrap_or(body);
    [body, newline].concat()
}

/// Whether `url` is a repository path rather than a URL, told apart as git does: no `scheme://` or
/// `transport::` prefix, and no ':' before the first '/' (`host:path` is scp-like ssh).
pub fn is_local_path(url: &OsStr) -> bool {
    let bytes = url.as_bytes();
    // ^[A-Za-z][A-Za-z0-9+.-]*(://|::)
    if bytes.first().is_some_and(u8::is_ascii_alphabetic) {
        let end = 1 + bytes[1..]
            .iter()
            .take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'.' | b'-'))
            .count();
        if bytes[end..].starts_with(b"://") || bytes[end..].starts_with(b"::") {
            return false;
        }
    }
    let colon = bytes.iter().position(|b| *b == b':');
    let slash = bytes.iter().position(|b| *b == b'/');
    match (colon, slash) {
        (None, _) => true,
        (Some(colon), Some(slash)) => slash < colon,
        (Some(_), None) => false,
    }
}

/// One decoded OS-text unit: valid UTF-8 text or an invalid byte retained unchanged.
enum Unit<'a> {
    Text(&'a str),
    Byte(u8),
}

fn units(bytes: &[u8]) -> Vec<Unit<'_>> {
    let mut out = Vec::new();
    for chunk in bytes.utf8_chunks() {
        if !chunk.valid().is_empty() {
            out.push(Unit::Text(chunk.valid()));
        }
        out.extend(chunk.invalid().iter().map(|b| Unit::Byte(*b)));
    }
    out
}

/// Whether `c` is Unicode whitespace or one of the separators U+001C to U+001F.
fn is_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// OS bytes without Unicode whitespace or separators U+001C to U+001F at either end.
/// A byte that is not UTF-8 is never whitespace.
pub fn strip_text_bytes(bytes: &[u8]) -> &[u8] {
    let mut start = 0;
    for chunk in bytes.utf8_chunks() {
        let valid = chunk.valid();
        let trimmed = valid.trim_start_matches(is_space);
        start += valid.len() - trimmed.len();
        if !trimmed.is_empty() || !chunk.invalid().is_empty() {
            break;
        }
    }
    let rest = &bytes[start..];
    // From the end: the last chunk's valid text, if nothing invalid follows it.
    let mut end = rest.len();
    let chunks: Vec<_> = rest.utf8_chunks().collect();
    for chunk in chunks.iter().rev() {
        if !chunk.invalid().is_empty() {
            break;
        }
        let valid = chunk.valid();
        let trimmed = valid.trim_end_matches(is_space);
        end -= valid.len() - trimmed.len();
        if !trimmed.is_empty() {
            break;
        }
    }
    &rest[..end]
}

/// OS bytes with each run of UTF-8 lowercased (final sigma included).
/// Bytes that are not UTF-8 stay as they are.
pub fn lowercase_text_bytes(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    for unit in units(bytes) {
        match unit {
            Unit::Text(text) => out.extend_from_slice(text.to_lowercase().as_bytes()),
            Unit::Byte(byte) => out.push(byte),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wp(url: &str) -> (String, bool) {
        let (out, dropped) = without_password(OsStr::new(url));
        (out.into_string().unwrap(), dropped)
    }

    fn norm(url: &str) -> String {
        normalize_remote(OsStr::new(url), Path::new("/nonexistent-cwd"))
            .into_string()
            .unwrap()
    }

    #[test]
    fn passwords_go_and_users_stay_as_git_reads_them() {
        assert_eq!(
            wp("ext::ssh://git:secret@host/x"),
            ("ext::ssh://git@host/x".into(), true)
        );
        assert_eq!(wp("a::b::c://u:p@h/x"), ("a::b::c://u:p@h/x".into(), false));
        assert_eq!(
            wp("ssh://user:pass/word@host/x"),
            ("ssh://user@host/x".into(), true)
        );
        assert_eq!(wp("ssh://u@h/a@b"), ("ssh://u@h/a@b".into(), false));
        assert_eq!(wp("https://h/a@b"), ("https://h/a@b".into(), false));
        assert_eq!(wp("https://[::1]x/a@b"), ("https://b".into(), true));
    }

    #[test]
    fn normalising_strips_lowers_and_reads_scp() {
        assert_eq!(norm("  Host:Org/Repo.git/ \n"), "host/org/repo");
        assert_eq!(norm("host:2222/x"), "host:2222/x");
        assert_eq!(norm("x.git\n"), "x");
        assert_eq!(norm("ΑΣ"), "ας");
    }

    #[test]
    fn local_paths_are_told_apart_from_urls() {
        for local in ["/srv/x", "x", "./a:b", "a/b:c"] {
            assert!(is_local_path(OsStr::new(local)), "{local}");
        }
        for url in ["ssh://h/x", "ext::x", "host:path", "u@host:path", "a:"] {
            assert!(!is_local_path(OsStr::new(url)), "{url}");
        }
    }

    #[test]
    fn bytes_that_are_not_text_survive_strip_and_lower() {
        assert_eq!(strip_text_bytes(b" \xff A \xfe "), b"\xff A \xfe");
        assert_eq!(lowercase_text_bytes(b"A\xffB\xce\xa3"), b"a\xffb\xcf\x82");
    }
}
