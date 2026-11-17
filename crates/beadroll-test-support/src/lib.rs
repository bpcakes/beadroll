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
mod gate;
mod isolation;
mod machine;
mod sandbox;
mod spawned;
pub mod values;
mod wait;

pub use beadroll_core::json::Value as Json;
pub use gate::{Gate, beside};
pub use isolation::assert_git_isolation;
pub use machine::{Invocation, Machine, Run, UNPROFILED, code};
pub use sandbox::{Sandbox, UNDER_TEST, built_bead, git, git_command, select_bead};
pub use spawned::{ProcessGroup, Spawned};
pub use wait::{REMOTE_SETTLES, wait_until, within};

/// Reads bead's output as a JSON document.
pub fn parse_json(text: &str) -> Result<Json, String> {
    beadroll_core::json::from_text(text).map_err(|error| error.to_string())
}

/// The `id` in what `bead … --json` printed, such as the issue a `create` made. The test fails,
/// saying which, when bead printed nothing (the command failed) or no id.
pub fn printed_id(printed: Option<Json>) -> String {
    let printed = printed.expect("bead printed nothing: the command failed");
    match printed.get("id").and_then(Json::as_str) {
        Some(id) => id.to_owned(),
        None => panic!("no id in what bead printed: {printed:?}"),
    }
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
    use super::{Json, parse_json, printed_id};

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
