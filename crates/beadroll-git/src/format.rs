//! The object format a repository of bead's own stores, read in-process from its `config`
//! (`extensions.objectformat`, `docs/RUST-PORT.md` Appendix B), where Python asks `git rev-parse
//! --show-object-format`.
//!
//! The reader follows only the plain form `git init` writes: section headers alone on their line,
//! `key = value` lines without quotes, escapes or comments after the value. Anything else is
//! [`StoredFormat::Unsure`], and so is a format this reader finds different from the one wanted:
//! the caller then asks git, whose answer decides as before. A config naming the wanted format is
//! enough to act on without git: if git disagreed it would be because it refuses the repository,
//! and bead then leaves it alone just the same.

use std::path::Path;

/// What a repository's `config` says of the object format it stores.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoredFormat {
    /// Nothing is there: git refuses to run in it.
    Absent,
    /// The format its config names (`sha1` when it names none), read from a plain config.
    Named(String),
    /// A config this reader does not follow to the letter, or none: ask git.
    Unsure,
}

/// The object format the repository at `git_dir` stores, as its config file says it.
pub fn stored_format(git_dir: &Path) -> StoredFormat {
    match std::fs::symlink_metadata(git_dir) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return StoredFormat::Absent;
        }
        Err(_) => return StoredFormat::Unsure,
    }
    match std::fs::read(git_dir.join("config")) {
        Ok(text) => named_format(&text).map_or(StoredFormat::Unsure, StoredFormat::Named),
        Err(_) => StoredFormat::Unsure,
    }
}

/// The format a plain config names: the last `objectformat` of a section `[extensions]` (names
/// read without regard to case, as git reads them), `sha1` when there is none; `None` for anything
/// this reader does not follow to the letter.
fn named_format(text: &[u8]) -> Option<String> {
    #[derive(PartialEq)]
    enum Section {
        None,
        Extensions,
        Other,
    }
    let mut section = Section::None;
    let mut format: Option<&[u8]> = None;
    for line in text.split(|b| *b == b'\n') {
        let line = trim(line);
        if line.is_empty() || line.starts_with(b"#") || line.starts_with(b";") {
            continue;
        }
        if let Some(header) = line.strip_prefix(b"[") {
            let name = header.strip_suffix(b"]")?;
            section = match name.iter().position(|b| *b == b' ') {
                None if plain_name(name) => {
                    if name.eq_ignore_ascii_case(b"extensions") {
                        Section::Extensions
                    } else {
                        Section::Other
                    }
                }
                // `[name "subsection"]`: never extensions.objectformat
                Some(at) if plain_name(&name[..at]) && plain_subsection(&name[at + 1..]) => {
                    Section::Other
                }
                _ => return None,
            };
            continue;
        }
        let key_end = line
            .iter()
            .position(|b| !(b.is_ascii_alphanumeric() || *b == b'-'))
            .unwrap_or(line.len());
        let (key, rest) = line.split_at(key_end);
        if !key.first().is_some_and(u8::is_ascii_alphabetic) || section == Section::None {
            return None;
        }
        let rest = trim(rest);
        let value = match rest.strip_prefix(b"=") {
            Some(value) => trim(value),
            None if rest.is_empty() => &b""[..],
            None => return None,
        };
        if value
            .iter()
            .any(|b| matches!(b, b'"' | b'\\' | b'#' | b';'))
        {
            return None;
        }
        if section == Section::Extensions && key.eq_ignore_ascii_case(b"objectformat") {
            format = Some(value);
        }
    }
    match format.unwrap_or(b"sha1") {
        b"sha1" => Some("sha1".to_owned()),
        b"sha256" => Some("sha256".to_owned()),
        _ => None,
    }
}

/// A section name as git writes one: letters, digits and dashes.
fn plain_name(name: &[u8]) -> bool {
    !name.is_empty() && name.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'-')
}

/// `"subsection"` with nothing git would unescape in it.
fn plain_subsection(text: &[u8]) -> bool {
    text.len() >= 2
        && text.starts_with(b"\"")
        && text.ends_with(b"\"")
        && !text[1..text.len() - 1]
            .iter()
            .any(|b| matches!(b, b'"' | b'\\'))
}

/// `line` without spaces, tabs and carriage returns at either end.
fn trim(line: &[u8]) -> &[u8] {
    let blank = |b: &u8| matches!(b, b' ' | b'\t' | b'\r');
    let start = line.iter().position(|b| !blank(b)).unwrap_or(line.len());
    let end = line
        .iter()
        .rposition(|b| !blank(b))
        .map_or(start, |at| at + 1);
    &line[start..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_config_names_its_format() {
        let sha256 = b"[core]\n\trepositoryformatversion = 1\n\tbare = true\n\
                       \tsharedrepository = 0600\n[extensions]\n\tobjectformat = sha256\n";
        assert_eq!(named_format(sha256).as_deref(), Some("sha256"));
        let sha1 = b"[core]\n\trepositoryformatversion = 0\n\tbare = true\n";
        assert_eq!(named_format(sha1).as_deref(), Some("sha1"));
        let cased = b"[Extensions]\n\tObjectFormat=sha256\n[remote \"x\"]\n\turl = /a/b\n";
        assert_eq!(named_format(cased).as_deref(), Some("sha256"));
        let subsection = b"[extensions \"x\"]\n\tobjectformat = sha256\n";
        assert_eq!(named_format(subsection).as_deref(), Some("sha1"));
    }

    #[test]
    fn anything_else_is_left_to_git() {
        for text in [
            &b"[extensions] objectformat = sha256\n"[..],
            b"objectformat = sha256\n",
            b"[extensions]\n\tobjectformat = \"sha256\"\n",
            b"[extensions]\n\tobjectformat = sha256 # x\n",
            b"[extensions]\n\tobjectformat = SHA256\n",
            b"[extensions]\n\tobjectformat\n",
            b"[extensions.x]\n\tobjectformat = sha256\n",
            b"\xef\xbb\xbf[core]\n",
            b"[core\n",
        ] {
            assert_eq!(named_format(text), None, "{text:?}");
        }
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            stored_format(&dir.path().join("none")),
            StoredFormat::Absent
        );
        assert_eq!(stored_format(dir.path()), StoredFormat::Unsure);
    }

    #[test]
    fn a_repository_git_made_names_the_format_git_reports() {
        for format in ["sha1", "sha256"] {
            let (dir, env) = crate::testing::bare(format);
            let reported = env.git(["rev-parse", "--show-object-format"]).string();
            assert_eq!(reported.unwrap(), format);
            let stored = stored_format(&dir.path().join("r.git"));
            assert_eq!(stored, StoredFormat::Named(format.to_owned()));
        }
    }
}
