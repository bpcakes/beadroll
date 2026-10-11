//! Where the caller works (Python's `capture_where`, `merged_into_default` and `object_format`):
//! the checkout's branch and commit, and what its repository knows about them.

use beadroll_core::json::{Object, Value};
use beadroll_git::{Fact, HeadAnswers, head_and_branch};

use crate::error::Error;
use crate::settings::Settings;
use crate::workspace::Workspace;

/// The branch and commit the caller is working on (`{"branch", "commit"}`), when the command runs
/// inside a checkout and the tracked configuration does not turn this off (`"where": false`). A
/// detached HEAD, or a branch name that is not UTF-8, records no branch. Asked of git each time, as
/// Python asks when each transaction starts: a branch switched meanwhile is the one recorded.
pub fn capture_where(ws: &Workspace) -> Result<Option<Value>, Error> {
    if ws.config.get("where") == Some(&Value::Bool(false)) {
        return Ok(None);
    }
    let HeadAnswers { head, branch } = head_and_branch(&ws.settings.git)?;
    // `rev-parse HEAD` failed (no commit here): no place, and its branch was not asked
    let (true, Some(branch)) = (head.success(), branch) else {
        return Ok(None);
    };
    let branch = std::str::from_utf8(&branch.stdout)
        .map(|text| beadroll_core::text::strip(text).to_owned())
        .unwrap_or_default();
    let commit = String::from_utf8_lossy(&head.stdout);
    let mut place = Object::new();
    let branch = if branch.is_empty() || branch == "HEAD" {
        Value::Null
    } else {
        Value::from(branch)
    };
    place.insert("branch".into(), branch);
    place.insert(
        "commit".into(),
        Value::from(beadroll_core::text::strip(&commit)),
    );
    Ok(Some(Value::from(place)))
}

/// Whether `commit` is reachable from the default branch as this checkout knows it: true when the
/// remote-tracking ref or the local branch contains it (a local branch in a worktree workflow is
/// often stale); `None` when the commit or the branch is unknown here.
pub fn merged_into_default(settings: &Settings, commit: &str) -> Result<Option<bool>, Error> {
    if commit.is_empty() {
        return Ok(None);
    }
    let git = &settings.git;
    let mut names: Vec<String> = Vec::new();
    let head = git
        .git(["symbolic-ref", "-q", "refs/remotes/origin/HEAD"])
        .run()?;
    if head.success() {
        let full = String::from_utf8_lossy(crate::strip_ascii(&head.stdout)).into_owned();
        names.push(
            full.strip_prefix("refs/remotes/origin/")
                .map_or(full.clone(), str::to_owned),
        );
    }
    for name in ["main", "master"] {
        if !names.iter().any(|n| n == name) {
            names.push(name.to_owned());
        }
    }
    for name in names {
        let mut refs = Vec::new();
        for candidate in [
            format!("refs/remotes/origin/{name}"),
            format!("refs/heads/{name}"),
        ] {
            if git
                .git(["rev-parse", "--verify", "-q", candidate.as_str()])
                .run()?
                .success()
            {
                refs.push(candidate);
            }
        }
        if refs.is_empty() {
            continue;
        }
        let mut codes = Vec::new();
        for target in &refs {
            let found = git
                .git(["merge-base", "--is-ancestor", commit, target.as_str()])
                .run()?;
            codes.push(found.returncode);
        }
        if codes.contains(&0) {
            return Ok(Some(true));
        }
        return Ok(if codes.contains(&1) {
            Some(false)
        } else {
            None
        });
    }
    Ok(None)
}

/// The object format (`sha1` or `sha256`) of the repository bead runs in; `None` outside one.
pub fn object_format(settings: &Settings) -> Result<Option<String>, Error> {
    let probe = settings.fact(Fact::ObjectFormat)?;
    let found = if probe.success() {
        probe.stdout_text()
    } else {
        String::new()
    };
    Ok(matches!(found.as_str(), "sha1" | "sha256").then_some(found))
}
