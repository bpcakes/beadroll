//! Black-box test fixtures: a [`Sandbox`] with a local bare remote and [`Machine`]s with
//! independent homes, clones and identities. Tracker writes stay within the sandbox.
//!
//! Binary selection is explicit: machine override ([`Machine::with_bead`]), then BEAD_UNDER_TEST,
//! then the test binary's default supplied to [`Sandbox::new`]. CLI tests pass
//! CARGO_BIN_EXE_bead; other crates use [`built_bead`]. A missing executable fails setup
//! immediately.
//!
//! [`Json`] reads bead's JSON output; [`json!`] and [`events`] build records for core integration
//! tests, and [`values`] draws the JSON values the round-trip properties write and read back.
#![allow(clippy::unwrap_used, reason = "a failed fixture is a failed test")]
#![allow(clippy::expect_used, reason = "a failed fixture is a failed test")]
#![allow(clippy::panic, reason = "a failed fixture is a failed test")]
#![allow(clippy::print_stderr, reason = "a failed fixture is a failed test")]

use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

pub mod events;
mod files;
mod gate;
mod isolation;
mod machine;
mod pool;
mod sandbox;
mod shell;
mod spawned;
pub mod values;
mod wait;

pub use beadroll_core::json::Value as Json;
pub use files::{copy_tree, descendants, segments, segments_in};
pub use gate::{Gate, beside};
pub use isolation::assert_git_isolation;
pub use machine::{
    Invocation, Machine, Run, UNPROFILED, code, converge, failure_code, reported_code,
};
pub use pool::pool_map;
pub use sandbox::{Sandbox, UNDER_TEST, built_bead, git, git_command};
pub use shell::{git_on_path, shell_quoted};
pub use spawned::{Drained, ProcessGroup, Spawned, drain};
pub use wait::{REMOTE_SETTLES, exited_within, wait_until, within};

/// Reads bead's output as a JSON document.
pub fn parse_json(text: &str) -> Result<Json, String> {
    beadroll_core::json::from_text(text).map_err(|error| error.to_string())
}

/// The `id` in what `bead … --json` printed, such as the issue a `create` made. The test fails,
/// saying which, when bead printed nothing (the command failed) or no id.
pub fn printed_id(printed: Option<Json>) -> String {
    json_id(&printed.expect("bead printed nothing: the command failed"))
}

/// The `id` member of a JSON document: an issue bead printed, a record of an export, an event of a
/// journal. The test fails, saying so, when it has none that is text.
pub fn json_id(value: &Json) -> String {
    match value.get("id").and_then(Json::as_str) {
        Some(id) => id.to_owned(),
        None => panic!("no id in what bead printed: {value:?}"),
    }
}

/// The `id` of each document of the array `documents`, in its order: the issues `ready` printed,
/// a list's `issues`, the records of an answer. The test fails when it is not an array or a
/// document has no id.
pub fn json_ids(documents: &Json) -> Vec<String> {
    let documents = documents
        .as_array()
        .unwrap_or_else(|| panic!("not a list of documents: {documents:?}"));
    documents.iter().map(json_id).collect()
}

/// The member `key` of a JSON object bead printed or a test read. The test fails, showing the
/// value, when it has none.
pub fn json_member<'v>(value: &'v Json, key: &str) -> &'v Json {
    value
        .get(key)
        .unwrap_or_else(|| panic!("no {key:?} in {value:?}"))
}

/// `value` as one line of JSON as bead writes it (ASCII, members in their order): a record for an
/// import, a body for a file under the store, an argument. The test fails if it cannot be written.
pub fn json_line(value: &Json) -> String {
    beadroll_core::json::write(value, beadroll_core::json::Format::ASCII)
        .unwrap_or_else(|error| panic!("{value:?} cannot be written: {error}"))
}

/// `path` as text, for an argument or a file a test writes it into. The sandbox's paths are UTF-8;
/// one that is not fails the test.
pub fn path_text(path: &Path) -> &str {
    path.to_str()
        .unwrap_or_else(|| panic!("{} is not UTF-8", path.display()))
}

/// The last line of `text` with anything but whitespace on it, or `""`: what a command's standard
/// error ends with, such as its one-line error.
pub fn last_line(text: &str) -> &str {
    text.lines()
        .rfind(|line| !line.trim().is_empty())
        .unwrap_or_default()
}

/// `rows` as JSON lines as bead reads them: each one [`json_line`] and a newline, a Beads export or
/// a journal's segment for a test to write to a file.
pub fn json_lines(rows: &[Json]) -> String {
    rows.iter().map(|row| json_line(row) + "\n").collect()
}

/// Whether the tests run as root, who reads and writes whatever a file's or directory's mode says,
/// so that a test of what a mode denies cannot run: it is skipped, saying `why` on standard error.
pub fn skipped_as_root(why: &str) -> bool {
    let root = rustix::process::geteuid().is_root();
    if root {
        eprintln!("skipped as root: {why}");
    }
    root
}

/// Writes `contents` to `path` as an executable (mode 0755): a hook, or a fake `git`, `ps` or
/// `ssh` a test puts on `PATH`.
pub fn executable(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) {
    let path = path.as_ref();
    std::fs::write(path, contents).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
}

#[cfg(test)]
mod tests {
    use super::{Json, last_line, parse_json, printed_id};

    /// The last line with text on it, wherever the blank lines are.
    #[test]
    fn the_last_line_is_the_last_one_with_text() {
        assert_eq!(last_line("one\ntwo\n\n  \n"), "two");
        assert_eq!(last_line("only"), "only");
        assert_eq!(last_line("\n \n"), "");
    }

    #[test]
    fn the_id_bead_printed_is_read_from_its_document() {
        let printed = parse_json(r#"{"id": "ex-1", "title": "t"}"#).ok();
        assert_eq!(printed_id(printed), "ex-1");
    }

    #[test]
    #[should_panic(expected = "bead printed nothing")]
    fn a_command_that_printed_nothing_fails_the_test() {
        printed_id(None);
    }

    #[test]
    #[should_panic(expected = "no id in what bead printed")]
    fn a_document_without_an_id_fails_the_test() {
        printed_id(Some(Json::Null));
    }
}
