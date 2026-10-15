//! The caller's checkout branch, commit and related repository facts.

use std::collections::{BTreeSet, HashMap, HashSet};

use beadroll_core::json::{Object, Value};
use beadroll_git::{Env, Fact, HeadAnswers, head_and_branch};

use crate::error::Error;
use crate::settings::Settings;
use crate::workspace::Workspace;

/// Records branch and commit when inside a checkout unless tracked configuration sets `where` to
/// false. Detached HEAD or a branch name that is not UTF-8 records no branch. Query git at every
/// transaction start so intervening branch changes are observed.
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

/// Whether each of `commits`, in their order, is reachable from the default branch as this
/// checkout knows it: true when the remote-tracking ref or the local branch contains it (a local
/// branch in a worktree workflow is often stale); `None` when the commit or the branch is unknown
/// here, and for an empty commit.
///
/// A reader that shows several closed issues asks once for all of them (roll-lqjj.26): the default
/// branch is found once and each distinct commit is answered once. Full object names, as bead
/// records the commit of a place, are answered together when there are several, in a fixed number
/// of gits ([`screened`]); any other text, a single commit, and those few that history too long to
/// walk leaves unsettled, are asked alone with `merge-base --is-ancestor`, as one commit always was.
pub fn merged_into_default(
    settings: &Settings,
    commits: &[&str],
) -> Result<Vec<Option<bool>>, Error> {
    let git = &settings.git;
    let asked: BTreeSet<&str> = (commits.iter().copied())
        .filter(|commit| !commit.is_empty())
        .collect();
    let mut answers: HashMap<&str, Option<bool>> = HashMap::new();
    let refs = if asked.is_empty() {
        Vec::new()
    } else {
        default_refs(git)?
    };
    if !refs.is_empty() {
        let (named, other): (Vec<&str>, Vec<&str>) = asked
            .into_iter()
            .partition(|commit| object_name(commit.as_bytes()));
        let mut alone = match named.len() {
            0 | 1 => named,
            _ => screened(git, &refs, named, &mut answers)?,
        };
        alone.extend(other);
        for commit in alone {
            answers.insert(commit, ancestor_of_any(git, &refs, commit)?);
        }
    }
    Ok((commits.iter())
        .map(|commit| answers.get(commit).copied().flatten())
        .collect())
}

/// The refs that stand for the default branch here: `refs/remotes/origin/NAME` and
/// `refs/heads/NAME`, those that exist, for the first NAME that has either of origin/HEAD's
/// branch, `main` and `master`; none when no NAME has.
fn default_refs(git: &Env) -> Result<Vec<String>, Error> {
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
        if !refs.is_empty() {
            return Ok(refs);
        }
    }
    Ok(Vec::new())
}

/// Whether `commit` is a full object name in lowercase hex, SHA-1 or SHA-256, as `rev-parse HEAD`
/// prints it: the commits answered together. git takes such a name as that object whatever refs
/// there are, so `cat-file` and `merge-base` read it alike; an abbreviation would not be, as
/// `^{commit}` makes git prefer a commit among the objects it could name.
fn object_name(commit: &[u8]) -> bool {
    matches!(commit.len(), 40 | 64)
        && (commit.iter()).all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// How many commits of the default branch's history [`screened`] walks for each commit it has
/// left to settle: walking one costs git a microsecond or a few, starting a git to ask one alone a
/// millisecond or more.
const WALKED_PER_COMMIT: usize = 1_000;

/// Answers each of `named`, distinct full object names, as [`ancestor_of_any`] would, into
/// `answers`, and returns those it leaves unsettled, to ask alone.
///
/// One `cat-file` peels each name to its commit, as `merge-base` does, and those that name none
/// are unknown here. One `rev-list` lists the commits no ref reaches, walking from them and from
/// the refs; those it does not list a ref contains, for git leaves out only what it found the refs
/// reach. Those it lists it may still be wrong about, because it stops walking the refs' history by
/// commit date, a few commits after all it has left to walk is what the refs reach, and where
/// dates disagree with history (a commit made while the clock was ahead) it can list a commit a
/// ref contains. So one more `rev-list` lists the refs' history itself, newest first, up to
/// [`WALKED_PER_COMMIT`] commits for each of them: one it lists a ref contains, and when it ends
/// before that bound it has listed the whole history, and the others no ref contains.
fn screened<'c>(
    git: &Env,
    refs: &[String],
    named: Vec<&'c str>,
    answers: &mut HashMap<&'c str, Option<bool>>,
) -> Result<Vec<&'c str>, Error> {
    let input: String = (named.iter())
        .map(|name| format!("{name}^{{commit}}\n"))
        .collect();
    let peeled = (git.git(["cat-file", "--batch-check=%(objectname)"]))
        .input(input)
        .run()?;
    // one line for each name: its commit's name, or the name and why there is none
    let commits: Vec<Option<&[u8]>> = (lines(&peeled.stdout))
        .map(|line| object_name(line).then_some(line))
        .collect();
    if !peeled.success() || commits.len() != named.len() {
        return Ok(named);
    }
    let mut input: Vec<u8> = (refs.iter())
        .flat_map(|target| format!("^{target}\n").into_bytes())
        .collect();
    for commit in commits.iter().flatten() {
        input.extend_from_slice(commit);
        input.push(b'\n');
    }
    let listed = git.git(["rev-list", "--stdin"]).input(input).run()?;
    if !listed.success() {
        return Ok(named);
    }
    let listed: HashSet<&[u8]> = lines(&listed.stdout).collect();
    let mut unsure = Vec::new();
    for (name, commit) in named.into_iter().zip(commits) {
        match commit {
            Some(commit) if listed.contains(commit) => unsure.push((name, commit)),
            Some(_) => {
                answers.insert(name, Some(true));
            }
            None => {
                answers.insert(name, None);
            }
        }
    }
    if unsure.is_empty() {
        return Ok(Vec::new());
    }
    let bound = WALKED_PER_COMMIT * unsure.len();
    let walk = [
        vec!["rev-list".into(), format!("--max-count={bound}")],
        refs.to_vec(),
    ]
    .concat();
    let history = git.git(&walk).run()?;
    if !history.success() {
        return Ok(unsure.into_iter().map(|(name, _)| name).collect());
    }
    let walked: HashSet<&[u8]> = lines(&history.stdout).collect();
    let whole = walked.len() < bound;
    let mut alone = Vec::new();
    for (name, commit) in unsure {
        if walked.contains(commit) {
            answers.insert(name, Some(true));
        } else if whole {
            answers.insert(name, Some(false));
        } else {
            alone.push(name);
        }
    }
    Ok(alone)
}

/// The lines of git's output, without their newlines.
fn lines(output: &[u8]) -> impl Iterator<Item = &[u8]> {
    output
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
}

/// What `merge-base --is-ancestor` says of `commit` against each of `refs`: contained when one
/// contains it, not when one says not and none contains it, unknown when git answered neither (a
/// commit unknown here, text that names none). `commit` is read as a revision, never as an option,
/// and text no argument can carry, with a NUL, names no commit.
fn ancestor_of_any(git: &Env, refs: &[String], commit: &str) -> Result<Option<bool>, Error> {
    if commit.contains('\0') {
        return Ok(None);
    }
    let mut codes = Vec::new();
    for target in refs {
        let ask = [
            "merge-base",
            "--is-ancestor",
            "--end-of-options",
            commit,
            target,
        ];
        codes.push(git.git(ask).run()?.returncode);
    }
    Ok(if codes.contains(&0) {
        Some(true)
    } else if codes.contains(&1) {
        Some(false)
    } else {
        None
    })
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
