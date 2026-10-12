//! A published journal, as the coordination ref's tree holds it (`docs/RUST-PORT.md` §2.4):
//! `manifest.json`, then each machine's segments `events/MACHINE/N.jsonl` and attempt files
//! `events/MACHINE/claims/ID.json`. The caller reads the entries from git; nothing here does.

use crate::code::Code;
use crate::event::FORMAT;
use crate::journal::{Journal, read_attempt, read_segment};
use crate::json::{self, Value};
use crate::text::{repr, str_of};

/// Why a published tree cannot be used, as `Link.fetch` refuses one.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum TreeError {
    /// No `manifest.json`.
    #[error("the published journal has no manifest; refusing to adopt it")]
    NoManifest,
    /// A manifest that is not a JSON object bead reads.
    #[error("the published journal has an unreadable manifest")]
    UnreadableManifest,
    /// A journal belonging to another workspace, with the reported workspace name.
    #[error("the published journal belongs to workspace {found}, not {expected}")]
    WorkspaceMismatch {
        /// The workspace the manifest names.
        found: String,
        /// The workspace the caller expects.
        expected: String,
    },
    /// A format other than the integer 1 (`true` and `1.0` are not format 1: §5 row 23).
    #[error(
        "the published journal is in format {0}; this version of bead reads format 1 only and will not rewrite it"
    )]
    Format(String),
}

impl TreeError {
    /// `MANIFEST_MISSING`, `WORKSPACE_MISMATCH` or `FORMAT_UNSUPPORTED`.
    pub fn code(&self) -> Code {
        match self {
            TreeError::NoManifest | TreeError::UnreadableManifest => Code::ManifestMissing,
            TreeError::WorkspaceMismatch { .. } => Code::WorkspaceMismatch,
            TreeError::Format(_) => Code::FormatUnsupported,
        }
    }
}

/// A file of the published tree that is a journal: its machine and whether it is an attempt.
fn journal_file(path: &str) -> Option<(&str, bool)> {
    let rest = path.strip_prefix("events/")?;
    let (replica, name) = rest.split_once('/')?;
    if replica.is_empty() {
        return None;
    }
    let (claims, file) = match name.split_once('/') {
        Some(("claims", file)) => (true, file),
        Some(_) => return None,
        None => (false, name),
    };
    let (stem, matches) = if claims {
        let stem = file.strip_suffix(".json")?;
        (
            stem,
            stem.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase() || b == b'-'),
        )
    } else {
        let stem = file.strip_suffix(".jsonl")?;
        (stem, stem.bytes().all(|b| b.is_ascii_digit()))
    };
    (!stem.is_empty() && matches).then_some((replica, claims))
}

/// A published journal, its manifest checked.
#[derive(Clone, Debug)]
pub struct Tree {
    workspace: String,
    /// Each journal file: its path, its machine, whether it is an attempt, and its bytes.
    files: Vec<(String, String, bool, Vec<u8>)>,
}

impl Tree {
    /// The tree of `entries` (each path and its bytes), refused as `Link.fetch` refuses it: without
    /// a readable manifest, for a workspace other than `expected`, or in a format other than 1.
    /// Paths other than the manifest and journal files are ignored.
    pub fn read(
        entries: impl IntoIterator<Item = (String, Vec<u8>)>,
        expected: &str,
    ) -> Result<Tree, TreeError> {
        let mut manifest = None;
        let mut files = Vec::new();
        for (path, bytes) in entries {
            if path == "manifest.json" {
                manifest = Some(bytes);
            } else if let Some((replica, attempt)) = journal_file(&path) {
                let replica = replica.to_owned();
                files.push((path, replica, attempt, bytes));
            }
        }
        let manifest = manifest.ok_or(TreeError::NoManifest)?;
        let parsed = json::parse(&manifest)
            .map_err(|_| TreeError::UnreadableManifest)?
            .value;
        let Value::Object(members) = &parsed else {
            return Err(TreeError::UnreadableManifest);
        };
        let found = members.get("workspace").cloned().unwrap_or(Value::Null);
        if found != Value::from(expected) {
            return Err(TreeError::WorkspaceMismatch {
                found: str_of(&found),
                expected: expected.to_owned(),
            });
        }
        let format = members.get("format").cloned().unwrap_or(Value::Null);
        if !matches!(&format, Value::Int(n) if n.as_i64() == Some(FORMAT)) {
            return Err(TreeError::Format(repr(&format)));
        }
        // Read each machine's segments followed by its attempt files, ordered by machine and then
        // path components.
        files.sort_by(|a, b| (a.2, &a.1, &a.0).cmp(&(b.2, &b.1, &b.0)));
        Ok(Tree {
            workspace: expected.to_owned(),
            files,
        })
    }

    /// The workspace it belongs to.
    pub fn workspace(&self) -> &str {
        &self.workspace
    }

    /// Its journal, read and settled as `load_journal` reads a store: a torn segment tail skipped,
    /// every unreadable line, attempt and record reported.
    pub fn journal(&self) -> Journal {
        let mut journal = Journal::new();
        for (path, replica, attempt, bytes) in &self.files {
            let events = if *attempt {
                read_attempt(bytes, path, Some(&mut journal.invalid))
            } else {
                read_segment(bytes, path, Some(&mut journal.invalid))
            };
            for event in events {
                journal.add(event, replica, path, !*attempt);
            }
        }
        journal.settle();
        journal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORKSPACE: &str = "01a11621-e285-7398-ab1c-b9af4864eaa2";

    fn manifest(text: &str) -> (String, Vec<u8>) {
        ("manifest.json".into(), text.as_bytes().to_vec())
    }

    fn entries(manifest_text: &str) -> Vec<(String, Vec<u8>)> {
        vec![manifest(manifest_text)]
    }

    fn refusal(entries: Vec<(String, Vec<u8>)>) -> TreeError {
        Tree::read(entries, WORKSPACE).err().unwrap()
    }

    #[test]
    fn a_manifest_is_read_as_link_fetch_reads_it() {
        let good = format!("{{\"format\": 1, \"workspace\": \"{WORKSPACE}\"}}\n");
        assert_eq!(
            Tree::read(entries(&good), WORKSPACE).unwrap().workspace(),
            WORKSPACE
        );
        assert_eq!(refusal(Vec::new()), TreeError::NoManifest);
        for unreadable in ["", "[1]", "{\"format\": NaN}", "{", "\"x\""] {
            assert_eq!(
                refusal(entries(unreadable)),
                TreeError::UnreadableManifest,
                "{unreadable}"
            );
        }
        let other = refusal(entries("{\"format\": 1, \"workspace\": \"elsewhere\"}"));
        assert_eq!(other.code(), Code::WorkspaceMismatch);
        assert_eq!(
            refusal(entries("{\"format\": 1}")).code(),
            Code::WorkspaceMismatch
        );
        // Only integer 1 denotes format 1. The refusal renders the supplied format with `repr`.
        for (format, shown) in [
            ("true", "True"),
            ("1.0", "1.0"),
            ("\"1\"", "'1'"),
            ("2", "2"),
            ("null", "None"),
        ] {
            let text = format!("{{\"format\": {format}, \"workspace\": \"{WORKSPACE}\"}}");
            assert_eq!(
                refusal(entries(&text)),
                TreeError::Format(shown.into()),
                "{format}"
            );
        }
        let missing = format!("{{\"workspace\": \"{WORKSPACE}\"}}");
        assert_eq!(refusal(entries(&missing)).code(), Code::FormatUnsupported);
    }

    #[test]
    fn the_journal_is_read_as_load_journal_reads_a_store() {
        let id = |n: u64| format!("019a5555-5555-7000-8000-{n:012x}");
        let line = |n: u64, replica: &str, deps: &str| {
            format!(
                "{{\"v\":1,\"id\":\"{}\",\"replica\":\"{replica}\",\"deps\":[{deps}],\"op\":\"comment\",\"data\":{{}}}}\n",
                id(n)
            )
        };
        let mut files = vec![
            manifest(&format!(
                "{{\"format\": 1, \"workspace\": \"{WORKSPACE}\"}}"
            )),
            (
                "events/b/000001.jsonl".into(),
                line(3, "b", "").into_bytes(),
            ),
            (
                "events/a/000002.jsonl".into(),
                format!("{}{{\"torn", line(2, "a", &format!("\"{}\"", id(1)))).into_bytes(),
            ),
            (
                "events/a/000001.jsonl".into(),
                line(1, "a", "").into_bytes(),
            ),
            (
                "events/a/claims/019a5555-5555-7000-8000-000000000009.json".into(),
                line(9, "a", "").into_bytes(),
            ),
        ];
        // Names a merge never copies: digits that are not ASCII, upper-case hex, other places.
        for path in [
            "events/a/\u{661}.jsonl",
            "events/a/claims/ABC.json",
            "events/a/x/1.jsonl",
            "notes.jsonl",
        ] {
            files.push((path.into(), line(7, "a", "").into_bytes()));
        }
        let tree = Tree::read(files, WORKSPACE).unwrap();
        let journal = tree.journal();
        let order: Vec<String> = journal
            .order()
            .get("a")
            .unwrap()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(order, [id(1), id(2)]); // its own order, the torn tail skipped
        assert!(journal.events().keys().any(|e| e.to_string() == id(9))); // the attempt, unsequenced
        assert!(!journal.events().keys().any(|e| e.to_string() == id(7)));
        assert!(journal.invalid.is_empty(), "{:?}", journal.invalid);
    }
}
