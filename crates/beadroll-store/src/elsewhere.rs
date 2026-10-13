//! Where a file kept on branches has changes this checkout lacks, for an import's dry run: Beads
//! keeps its tracker in `.beads/issues.jsonl` on every branch, so work recorded on another branch,
//! or in another worktree and not yet committed, is not in this checkout's copy.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt as _;

use beadroll_core::text::printed_text;

use crate::error::Error;
use crate::settings::Settings;

/// Changes to a file the checkout lacks.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Elsewhere {
    /// Each branch, local or remote-tracking, with commits that touch the file and that HEAD does
    /// not have, by its full name, and how many.
    pub refs: Vec<(String, u64)>,
    /// Each other worktree of the repository whose copy of the file has changes not committed.
    pub worktrees: Vec<String>,
}

/// Where `path`, relative to the top of the checkout, has changes the checkout lacks: none outside
/// a repository, or before its first commit. A name that is not UTF-8 is shown with `?` for each
/// byte that is not.
pub fn elsewhere(settings: &Settings, path: &str) -> Result<Elsewhere, Error> {
    let git = &settings.git;
    let mut found = Elsewhere::default();
    // from the top of the checkout, wherever in it bead runs
    let pathspec = format!(":(top){path}");
    if !git
        .git(["rev-parse", "--verify", "-q", "HEAD"])
        .run()?
        .success()
    {
        return Ok(found);
    }
    let listed = git
        .git([
            "for-each-ref",
            "--format=%(refname)%00%(symref)",
            "refs/heads",
            "refs/remotes",
        ])
        .run()?;
    for line in listed.stdout.split(|byte| *byte == b'\n') {
        // a symbolic ref (origin's HEAD) names a branch listed in its own right
        let Some((name, b"")) = split_once(line, 0) else {
            continue;
        };
        let range = [b"HEAD..", name].concat();
        let counted = git
            .git([
                OsStr::new("rev-list"),
                OsStr::new("--count"),
                OsStr::from_bytes(&range),
                OsStr::new("--"),
                OsStr::new(&pathspec),
            ])
            .run()?;
        let count = std::str::from_utf8(crate::strip_ascii(&counted.stdout))
            .ok()
            .and_then(|count| count.parse::<u64>().ok());
        if let Some(count) = count.filter(|count| counted.success() && *count > 0) {
            found.refs.push((printed_text(name), count));
        }
    }
    let top = git.git(["rev-parse", "--show-toplevel"]).run()?;
    let top = crate::strip_ascii(&top.stdout);
    let worktrees = git.git(["worktree", "list", "--porcelain", "-z"]).run()?;
    for field in worktrees.stdout.split(|byte| *byte == 0) {
        let Some(tree) = field.strip_prefix(b"worktree ") else {
            continue;
        };
        if tree == top {
            continue;
        }
        // read only: without optional locks, status never writes the refreshed index back
        let status = git
            .git([
                OsStr::new("--no-optional-locks"),
                OsStr::new("-C"),
                OsStr::from_bytes(tree),
                OsStr::new("status"),
                OsStr::new("--porcelain"),
                OsStr::new("--"),
                OsStr::new(path),
            ])
            .run()?;
        // a worktree git cannot read (moved, pruned) has nothing to say
        if status.success() && !crate::strip_ascii(&status.stdout).is_empty() {
            found.worktrees.push(printed_text(tree));
        }
    }
    Ok(found)
}

/// `bytes` split at the first `separator`.
fn split_once(bytes: &[u8], separator: u8) -> Option<(&[u8], &[u8])> {
    let at = bytes.iter().position(|byte| *byte == separator)?;
    Some((bytes.get(..at)?, bytes.get(at + 1..)?))
}
