//! The journal and its sync: an append cut short, a journal forked by a rewritten or a copied
//! history, rotation under parallel writers, a remote that lost its history, lost acknowledgements,
//! and git stalled in a push or a fetch. Its tests are grouped by subject in the modules below;
//! their shared helpers are here. The kill in the middle of an append and the exhaustion of a
//! command's retries are `rust_only`: they stop the Rust `bead` at its `append.mid` failpoint, and
//! skip its waits between rounds at `backoff`.

mod forks;
mod lost_acks;
mod remote_ref;
mod segments;
mod stalls;

use std::ffi::{OsStr, OsString};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use beadroll_test_support::executable;
use beadroll_test_support::{Machine, Sandbox, git};

/// Where a test wrapper pauses relative to its git command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HookPosition {
    /// The wrapper marks and waits before git runs: the operation has not happened.
    Before,
    /// Git runs first, then the wrapper marks and waits: the operation happened, its answer withheld.
    After,
}

/// How a test makes its machines: `Machine::new`, or `crate::rust_only` for a test that injects its
/// fault through a failpoint.
type Make = fn(&Sandbox, &str) -> Machine;

/// Machine a, its tracker initialised with the prefix `audit`, and the key of
/// the one issue it holds, `Original issue`.
fn setup(sandbox: &Sandbox) -> (Machine, String) {
    setup_with(sandbox, Machine::new)
}

/// Sets up machine a using `make`.
fn setup_with(sandbox: &Sandbox, make: Make) -> (Machine, String) {
    let a = make(sandbox, "a");
    a.bead(["init", "--prefix", "audit"]);
    let created = a.bead(["create", "Original issue"]).expect("the issue");
    let key = created["id"].as_str().expect("the issue's id").to_owned();
    (a, key)
}

/// `two`: a publishes its tracker, and a second machine, b, syncs it.
fn two(sandbox: &Sandbox, a: &Machine) -> Machine {
    two_with(sandbox, a, Machine::new)
}

/// `two` with machine b made by `make`.
fn two_with(sandbox: &Sandbox, a: &Machine, make: Make) -> Machine {
    a.bead(["sync"]);
    let b = make(sandbox, "b");
    b.bead(["sync"]);
    b
}

/// `own_segment`: the last segment of `machine`'s own journal, the one its next event is appended
/// to, `events/<machine>/*.jsonl` in its store by name.
fn own_segment(machine: &Machine) -> PathBuf {
    let info = machine.bead(["info"]).expect("bead info prints a record");
    let own = info["machine"].as_str().expect("the machine's id");
    let mut found = jsonl(&machine.store().join("events").join(own));
    found.sort();
    found.pop().expect("a segment of the machine's own journal")
}

/// The entries of `dir` whose names end with `.jsonl`, in directory order.
fn jsonl(dir: &Path) -> Vec<PathBuf> {
    let entries = std::fs::read_dir(dir).expect("a journal's folder");
    entries
        .map(|entry| entry.expect("a directory entry").path())
        .filter(|path| {
            let name = path.file_name().expect("a name");
            name.as_encoded_bytes().ends_with(b".jsonl")
        })
        .collect()
}

/// Runs git against the sandbox remote and returns stdout.
fn remote_git(sandbox: &Sandbox, args: &[&str]) -> String {
    let words = [OsStr::new("--git-dir"), sandbox.origin().as_os_str()];
    git(words.into_iter().chain(args.iter().map(OsStr::new)), None)
}

/// `remote_blob`: the file `path` of the journal as the remote's `refs/beads/journal` holds it.
fn remote_blob(sandbox: &Sandbox, path: &str) -> String {
    remote_git(sandbox, &["show", &format!("refs/beads/journal:{path}")])
}

/// A git wrapper delays push or fetch for 20 seconds. With `after`, run the operation before
/// creating `marker` and waiting, simulating a lost acknowledgement; otherwise mark and wait
/// first. Other git calls pass through. The shell's sleep child can outlive the client killed on
/// push timeout and finishes after its 20 seconds.
fn git_wrapper(sandbox: &Sandbox, action: &str, marker: &Path, after: HookPosition) -> OsString {
    let folder = sandbox.root().join(format!("wrapper-{action}"));
    std::fs::create_dir_all(&folder).unwrap();
    let (git, marker) = (quoted(&real_git()), quoted(marker));
    let held = if after == HookPosition::After {
        format!(
            "  {git} \"$@\"\n  status=$?\n  printf ready > {marker}\n  sleep 20\n  exit $status\n"
        )
    } else {
        format!("  printf ready > {marker}\n  sleep 20\n")
    };
    // Only delay the specified transport operation. All other git commands remain real.
    let script =
        format!("#!/bin/sh\nif [ \"$1\" = {action} ]; then\n{held}fi\nexec {git} \"$@\"\n");
    let wrapper = folder.join("git");
    executable(&wrapper, script);
    let mut path = folder.into_os_string();
    path.push(":");
    path.push(std::env::var_os("PATH").expect("the test's PATH"));
    path
}

/// The git executable on the test's `PATH`: the one the wrapper runs.
fn real_git() -> PathBuf {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let executable = |file: &Path| {
        let mode = std::fs::metadata(file).map(|meta| (meta.is_file(), meta.permissions().mode()));
        mode.is_ok_and(|(is_file, mode)| is_file && mode & 0o111 != 0)
    };
    std::env::split_paths(&path)
        .map(|dir| dir.join("git"))
        .find(|git| executable(git))
        .expect("git on PATH")
}

/// `path` quoted for the shell.
fn quoted(path: &Path) -> String {
    let text = path.to_str().expect("a UTF-8 path");
    format!("'{}'", text.replace('\'', r"'\''"))
}
