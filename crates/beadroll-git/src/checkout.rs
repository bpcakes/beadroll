//! Checkout facts with minimal git invocations (`crates/beadroll-git/AGENTS.md`). One combined
//! rev-parse obtains the repository top, common directory and object format once per process.
//! Split answers only when unambiguous, preserving each standalone call's output; otherwise ask
//! each fact separately when needed. These immutable facts use the first query's environment.
//!
//! Mutable configuration is never cached: read origin URLs whenever finding the workspace,
//! transport settings whenever creating a link, and HEAD and branch whenever starting a
//! transaction under the store lock. Configuration and checkout changes while waiting therefore
//! affect the command.

use std::sync::OnceLock;

use crate::error::Error;
use crate::runner::{Env, Git, Output};

/// The keys that hold origin's URLs (`git config --get-regexp`).
pub const ORIGIN_KEYS: &str = r"^remote\.origin\.(push)?url$";

/// The git configuration query for origin URLs, without `--null`.
const ORIGIN_ARGS: [&str; 3] = ["config", "--get-regexp", ORIGIN_KEYS];

/// An immutable checkout fact queried through git rev-parse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fact {
    /// `--show-toplevel`: the top of the working tree.
    Toplevel,
    /// `--path-format=absolute --git-common-dir`: the repository's common directory.
    CommonDir,
    /// `--show-object-format`: `sha1` or `sha256`.
    ObjectFormat,
}

impl Fact {
    /// Every fact, in the order the combined `rev-parse` prints them.
    const ALL: [Fact; 3] = [Fact::Toplevel, Fact::CommonDir, Fact::ObjectFormat];

    /// Arguments for querying this fact alone.
    pub fn args(self) -> &'static [&'static str] {
        match self {
            Fact::Toplevel => &["rev-parse", "--show-toplevel"],
            Fact::CommonDir => &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            Fact::ObjectFormat => &["rev-parse", "--show-object-format"],
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// The checkout's facts, each asked of git at most once.
#[derive(Clone, Debug, Default)]
pub struct Checkout {
    /// The combined `rev-parse`'s lines, one a fact; `None` when it cannot answer for them.
    combined: OnceLock<Option<Vec<Vec<u8>>>>,
    /// Facts asked on their own.
    single: [OnceLock<Result<Output, Error>>; 3],
}

impl Checkout {
    /// Queries ORIGIN_KEYS with git config whenever the workspace is found, and runs `also`, a
    /// local question the caller may need answered there (the committed `.beadroll.json`), beside
    /// it: origin's answer, and `also`'s. The first query runs beside the combined rev-parse; none
    /// waits for another.
    pub fn origin_urls(
        &self,
        env: &Env,
        also: Option<Git<'_>>,
    ) -> (Result<Output, Error>, Option<Result<Output, Error>>) {
        let origin = env.git(ORIGIN_ARGS);
        if self.combined.get().is_some() {
            return match also {
                None => (origin.run(), None),
                Some(also) => {
                    let (origin, [also]) = origin.run_alongside([also]);
                    (origin, Some(also))
                }
            };
        }
        let rev_parse = env.git(rev_parse_args());
        let (rev_parse, origin, also) = match also {
            None => {
                let (rev_parse, [origin]) = rev_parse.run_alongside([origin]);
                (rev_parse, origin, None)
            }
            Some(also) => {
                let (rev_parse, [origin, also]) = rev_parse.run_alongside([origin, also]);
                (rev_parse, origin, Some(also))
            }
        };
        let _ = self.combined.set(combined_rev_parse(rev_parse));
        (origin, also)
    }

    /// What `git rev-parse` prints for `fact` alone, in `env`: its exit status and output.
    pub fn fact(&self, env: &Env, fact: Fact) -> Result<Output, Error> {
        let combined = self
            .combined
            .get_or_init(|| combined_rev_parse(env.git(rev_parse_args()).run()));
        if let Some(line) = combined.as_ref().and_then(|lines| lines.get(fact.index())) {
            let mut stdout = line.clone();
            stdout.push(b'\n');
            return Ok(success(stdout));
        }
        // every fact has its place: `index` is below `Fact::ALL.len()`
        self.single[fact.index()]
            .get_or_init(|| env.git(fact.args()).run())
            .clone()
    }
}

/// The combined `rev-parse`: every fact's arguments, in [`Fact::ALL`]'s order.
fn rev_parse_args() -> Vec<&'static str> {
    let mut args = vec!["rev-parse"];
    for fact in Fact::ALL {
        args.extend(fact.args().iter().skip(1));
    }
    args
}

/// A call that succeeded, printing `stdout`.
fn success(stdout: Vec<u8>) -> Output {
    Output {
        returncode: 0,
        stdout,
        stderr: Vec::new(),
    }
}

/// The lines of the combined `rev-parse`, which answered `answer`, when it succeeded and each line
/// can only be the fact it stands for: three of them, an absolute top and common directory and a
/// known format. A path holding a newline would make more lines than three.
fn combined_rev_parse(answer: Result<Output, Error>) -> Option<Vec<Vec<u8>>> {
    let lines = answer_lines(answer)?;
    let [top, common, format] = lines.as_slice() else {
        return None;
    };
    let absolute = top.starts_with(b"/") && common.starts_with(b"/");
    let known = matches!(format.as_slice(), b"sha1" | b"sha256");
    (absolute && known).then_some(lines)
}

/// The lines of a call that succeeded, each without its newline; `None` for a call that failed or
/// whose output does not end in one.
fn answer_lines(answer: Result<Output, Error>) -> Option<Vec<Vec<u8>>> {
    let output = answer.ok()?;
    if !output.success() {
        return None;
    }
    let text = output.stdout.strip_suffix(b"\n")?;
    Some(text.split(|b| *b == b'\n').map(<[u8]>::to_vec).collect())
}

/// HEAD and branch from `rev-parse HEAD` followed, only on success, by `rev-parse --abbrev-ref
/// HEAD`, retaining each command's output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeadAnswers {
    /// `rev-parse HEAD`: the commit checked out.
    pub head: Output,
    /// `rev-parse --abbrev-ref HEAD`, asked once `head` succeeded: its branch, or `HEAD` when
    /// detached.
    pub branch: Option<Output>,
}

/// HEAD's full object name followed by its branch, queried together.
const HEAD_ARGS: [&str; 4] = ["rev-parse", "HEAD", "--abbrev-ref", "HEAD"];

/// Reads HEAD and branch afresh when a transaction starts under the store lock, including every
/// ownership round. A combined `rev-parse HEAD --abbrev-ref HEAD` is used only when its lines are
/// unambiguous: a 40- or 64-digit lowercase hex object name, then branch. Otherwise fall back to
/// the separate queries in order, covering unborn HEAD, ambiguous branch names and absent
/// repositories.
pub fn head_and_branch(env: &Env) -> Result<HeadAnswers, Error> {
    if let Some([head, branch]) = answer_lines(env.git(HEAD_ARGS).run()).as_deref() {
        let full_name = matches!(head.len(), 40 | 64)
            && head.iter().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        if full_name && !branch.is_empty() {
            let printed = |line: &[u8]| success([line, b"\n"].concat());
            return Ok(HeadAnswers {
                head: printed(head),
                branch: Some(printed(branch)),
            });
        }
    }
    let head = env.git(["rev-parse", "HEAD"]).run()?;
    let branch = if head.success() {
        Some(env.git(["rev-parse", "--abbrev-ref", "HEAD"]).run()?)
    } else {
        None
    };
    Ok(HeadAnswers { head, branch })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `git ARGS` in `dir` with the test's clean environment, which must succeed.
    fn git(env: &Env, args: &[&str]) {
        env.git(args).check().unwrap();
    }

    /// Checkouts of every kind, by name, in a temporary home: a commit on a branch, a detached
    /// HEAD, no commit yet, a branch named `HEAD` beside HEAD (which `--abbrev-ref` finds
    /// ambiguous), a bare repository, no repository at all, and SHA-256.
    fn checkouts(home: &std::path::Path) -> Vec<(&'static str, Env)> {
        let make = |name: &str| {
            let dir = home.join(name);
            std::fs::create_dir(&dir).unwrap();
            crate::testing::clean_env(Some(&dir), home, &[])
        };
        let identity = ["-c", "user.name=t", "-c", "user.email=t@example.invalid"];
        let commit = |env: &Env| {
            let args = identity
                .into_iter()
                .chain(["commit", "-q", "--allow-empty", "-m", "m"]);
            env.git(args).check().unwrap();
        };
        let committed = make("committed");
        git(&committed, &["init", "-q", "-b", "main"]);
        commit(&committed);
        let detached = make("detached");
        git(&detached, &["init", "-q", "-b", "main"]);
        commit(&detached);
        git(&detached, &["checkout", "-q", "--detach"]);
        let unborn = make("unborn");
        git(&unborn, &["init", "-q", "-b", "main"]);
        let ambiguous = make("ambiguous");
        git(&ambiguous, &["init", "-q", "-b", "main"]);
        commit(&ambiguous);
        git(&ambiguous, &["update-ref", "refs/heads/HEAD", "main"]);
        let bare = make("bare");
        git(&bare, &["init", "-q", "--bare"]);
        let nowhere = make("nowhere");
        let sha256 = make("sha256");
        git(
            &sha256,
            &["init", "-q", "-b", "main", "--object-format=sha256"],
        );
        commit(&sha256);
        vec![
            ("committed", committed),
            ("detached", detached),
            ("unborn", unborn),
            ("ambiguous", ambiguous),
            ("bare", bare),
            ("nowhere", nowhere),
            ("sha256", sha256),
        ]
    }

    #[test]
    fn every_fact_answers_as_its_own_call_in_every_kind_of_checkout() {
        let root = tempfile::tempdir().unwrap();
        for (name, env) in checkouts(root.path()) {
            let checkout = Checkout::default();
            for fact in Fact::ALL {
                let single = env.git(fact.args()).run().unwrap();
                let found = checkout.fact(&env, fact).unwrap();
                assert_eq!(found, single, "{fact:?} in {name}");
            }
            // the combined call answers in a working tree, commit or none, and cuts its answer
            // apart exactly; a bare repository and none at all ask each fact on its own
            let answered = matches!(checkout.combined.get(), Some(Some(_)));
            assert_eq!(answered, !matches!(name, "bare" | "nowhere"), "{name}");
        }
    }

    #[test]
    fn head_and_branch_match_separate_git_calls_and_refresh_each_time() {
        let root = tempfile::tempdir().unwrap();
        for (name, env) in checkouts(root.path()) {
            let head = env.git(["rev-parse", "HEAD"]).run().unwrap();
            let branch = head.success().then(|| {
                env.git(["rev-parse", "--abbrev-ref", "HEAD"])
                    .run()
                    .unwrap()
            });
            let expected = HeadAnswers { head, branch };
            assert_eq!(head_and_branch(&env).unwrap(), expected, "{name}");
            // one rev-parse answers both wherever its two lines are certain
            let combined = answer_lines(env.git(HEAD_ARGS).run()).is_some_and(|l| l.len() == 2);
            let certain = matches!(name, "committed" | "detached" | "sha256");
            assert_eq!(combined, certain, "{name}");
        }
        // asked again after the checkout moved: the new branch, nothing kept
        let moved = root.path().join("committed");
        let env = crate::testing::clean_env(Some(&moved), root.path(), &[]);
        git(&env, &["checkout", "-q", "-b", "moved"]);
        let branch = head_and_branch(&env).unwrap().branch.unwrap();
        assert_eq!(branch.stdout, b"moved\n");
    }

    #[test]
    fn origin_and_transport_settings_match_git_and_refresh_each_time() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path();
        let dir = home.join("w");
        std::fs::create_dir(&dir).unwrap();
        let env = crate::testing::clean_env(Some(&dir), home, &[]);
        git(&env, &["init", "-q"]);
        let transport = ["config", "--null", "--get-regexp", crate::TRANSPORT_KEYS];
        let checkout = Checkout::default();
        // the first question of a checkout without a commit yet: the combined rev-parse runs
        // beside origin's call and the caller's own question, and answers the facts
        let committed = || {
            env.git(["cat-file", "--batch"])
                .input(&b"HEAD:.beadroll.json\n"[..])
        };
        let (found, also) = checkout.origin_urls(&env, Some(committed()));
        let found = found.unwrap();
        assert_eq!(found, env.git(ORIGIN_ARGS).run().unwrap());
        assert_eq!(also.unwrap().unwrap(), committed().run().unwrap());
        assert_eq!(found.returncode, 1, "nothing set");
        assert!(matches!(checkout.combined.get(), Some(Some(_))));
        let top = env.git(Fact::Toplevel.args()).run().unwrap();
        assert_eq!(checkout.fact(&env, Fact::Toplevel).unwrap(), top);
        let settings = crate::transport_settings(&env).unwrap();
        assert_eq!(settings, env.git(transport).run().unwrap());
        // set afterwards, both are what the next question finds: nothing is kept
        let local = "[remote \"origin\"]\n\turl = ../o.git\n\tpushurl = \"a b\"\n\tpushurl\n\
                     [remote \"Origin\"]\n\turl = /x\n[url \"git@h:\"]\n\tinsteadOf = h:\n";
        let config = dir.join(".git/config");
        let mut text = std::fs::read_to_string(&config).unwrap();
        text.push_str(local);
        std::fs::write(&config, text).unwrap();
        let (found, also) = checkout.origin_urls(&env, None);
        assert!(also.is_none());
        let found = found.unwrap();
        assert_eq!(found, env.git(ORIGIN_ARGS).run().unwrap());
        let (again, also) = checkout.origin_urls(&env, Some(committed()));
        assert_eq!(
            (again.unwrap(), also.unwrap().unwrap()),
            (found.clone(), committed().run().unwrap())
        );
        assert_eq!(
            found.stdout,
            b"remote.origin.url ../o.git\nremote.origin.pushurl a b\nremote.origin.pushurl\n"
        );
        let settings = crate::transport_settings(&env).unwrap();
        assert_eq!(settings, env.git(transport).run().unwrap());
        assert_eq!(settings.stdout, b"url.git@h:.insteadof\nh:\0");
    }

    #[test]
    fn each_fact_uses_its_expected_git_arguments() {
        let args: Vec<&[&str]> = Fact::ALL.iter().map(|f| f.args()).collect();
        assert_eq!(
            args,
            [
                &["rev-parse", "--show-toplevel"][..],
                &["rev-parse", "--path-format=absolute", "--git-common-dir"],
                &["rev-parse", "--show-object-format"],
            ]
        );
        assert!(Fact::ALL.iter().enumerate().all(|(at, f)| f.index() == at));
    }
}
