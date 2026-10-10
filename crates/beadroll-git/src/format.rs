//! The object format a repository of bead's own stores, read in-process from its `config`
//! (`extensions.objectformat`, `docs/RUST-PORT.md` Appendix B), where Python asks `git rev-parse
//! --show-object-format`.
//!
//! The reader follows only the plain form `git init` writes: section headers alone on their line,
//! `key = value` lines without quotes, escapes or comments after the value. It names a format only
//! where git's own reading of the config does: `extensions.objectformat`, or SHA-1 when there is
//! none, in a repository of format version 1, and SHA-1 in one of version 0 that names no format.
//! Without `core.repositoryformatversion` git reads the config without its extensions, so a SHA-256
//! repository whose config lost that line is SHA-1 to git. Anything else is
//! [`StoredFormat::Unsure`], and so is a format this reader finds different from the one wanted:
//! the caller then asks git, whose answer decides as before. A config naming the wanted format is
//! then enough to act on without git: if git disagreed it would be because it refuses the
//! repository (an extension it does not know, say), and bead then leaves it alone just the same.

use std::path::Path;

/// What a repository's `config` says of the object format it stores.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoredFormat {
    /// Nothing is there: git refuses to run in it.
    Absent,
    /// The format git reads from its plain config.
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

/// The format git reads from a plain config: the last `objectformat` of a section `[extensions]`,
/// `sha1` when there is none, where the last `repositoryformatversion` of a section `[core]` is 1;
/// `sha1` where it is 0 and no format is named (names read without regard to case, as git reads
/// them). `None` for anything else: no version, which makes git pass over the extensions; version 0
/// with a format named, or another version, which git refuses; anything this reader does not
/// follow to the letter.
fn named_format(text: &[u8]) -> Option<String> {
    #[derive(PartialEq)]
    enum Section {
        None,
        Core,
        Extensions,
        Other,
    }
    let mut section = Section::None;
    let mut format: Option<&[u8]> = None;
    let mut version: Option<&[u8]> = None;
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
                    } else if name.eq_ignore_ascii_case(b"core") {
                        Section::Core
                    } else {
                        Section::Other
                    }
                }
                // `[name "subsection"]`: never extensions.objectformat or
                // core.repositoryformatversion
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
        if section == Section::Core && key.eq_ignore_ascii_case(b"repositoryformatversion") {
            version = Some(value);
        }
    }
    let named = match (version, format) {
        (Some(b"1"), format) => format.unwrap_or(b"sha1"),
        (Some(b"0"), None) => b"sha1",
        _ => return None,
    };
    match named {
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
        let cased = b"[Extensions]\n\tObjectFormat=sha256\n[remote \"x\"]\n\turl = /a/b\n\
                      [Core]\n\tRepositoryFormatVersion=1\n";
        assert_eq!(named_format(cased).as_deref(), Some("sha256"));
        let subsection =
            b"[extensions \"x\"]\n\tobjectformat = sha256\n[core]\n\trepositoryformatversion = 1\n";
        assert_eq!(named_format(subsection).as_deref(), Some("sha1"));
    }

    #[test]
    fn anything_else_is_left_to_git() {
        // each with a version that would let git read its format, so that what is left to git is
        // the rest of it
        let v1 = b"[core]\n\trepositoryformatversion = 1\n";
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
            let text = [text, v1].concat();
            assert_eq!(named_format(&text), None, "{text:?}");
        }
        // what the version leaves to git: none (git passes over the extensions), 0 with a format
        // named (git refuses it), any other, or one in another form
        let sha256 = "[extensions]\n\tobjectformat = sha256\n";
        for core in [
            "[core]\n\tbare = true\n",
            "[core]\n\trepositoryformatversion = 0\n",
            "[core]\n\trepositoryformatversion = 2\n",
            "[core]\n\trepositoryformatversion = 01\n",
            "[core]\n\trepositoryformatversion\n",
            "[core \"x\"]\n\trepositoryformatversion = 1\n",
        ] {
            let text = format!("{core}{sha256}");
            assert_eq!(named_format(text.as_bytes()), None, "{text:?}");
        }
        let unversioned = b"[core]\n\tbare = true\n";
        assert_eq!(named_format(unversioned), None);
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

    /// Repositories git made, their configs given each format version a config can carry: the
    /// reader names a format only where git reads that one, and leaves the rest to git. A SHA-256
    /// repository whose config lost its version is SHA-1 to git, which passes over its extensions.
    #[test]
    fn a_format_is_named_only_where_git_reads_it() {
        for format in ["sha1", "sha256"] {
            for version in [None, Some("0"), Some("1"), Some("01"), Some("2")] {
                let (dir, env) = crate::testing::bare(format);
                let git_dir = dir.path().join("r.git");
                let config = git_dir.join("config");
                let text = std::fs::read_to_string(&config).unwrap();
                let mut kept: String = text
                    .lines()
                    .filter(|line| !line.trim().starts_with("repositoryformatversion"))
                    .map(|line| format!("{line}\n"))
                    .collect();
                if let Some(version) = version {
                    kept.push_str(&format!("[core]\n\trepositoryformatversion = {version}\n"));
                }
                std::fs::write(&config, kept).unwrap();
                let git = env
                    .git(["rev-parse", "--show-object-format"])
                    .run()
                    .unwrap();
                let case = format!("{format}, version {version:?}");
                let expected = match (format, version) {
                    ("sha1", Some("0" | "1")) | ("sha256", Some("1")) => {
                        let why = String::from_utf8_lossy(&git.stderr);
                        assert!(git.success(), "{case}: {why}");
                        assert_eq!(git.stdout_text(), format, "{case}");
                        StoredFormat::Named(format.to_owned())
                    }
                    _ => StoredFormat::Unsure,
                };
                assert_eq!(stored_format(&git_dir), expected, "{case}");
            }
        }
        // the case that made it matter: git reads SHA-1 where the config names SHA-256
        let (dir, env) = crate::testing::bare("sha256");
        let config = dir.path().join("r.git").join("config");
        let text = std::fs::read_to_string(&config).unwrap();
        let unversioned = text.replace("\trepositoryformatversion = 1\n", "");
        assert_ne!(unversioned, text);
        std::fs::write(&config, unversioned).unwrap();
        let reported = env.git(["rev-parse", "--show-object-format"]).string();
        assert_eq!(reported.unwrap(), "sha1");
    }
}
