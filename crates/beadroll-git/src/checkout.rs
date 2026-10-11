//! What the checkout's git says about where bead runs, asked as few times as it can be
//! (`docs/RUST-PORT.md` Appendix B).
//!
//! Python runs one `rev-parse` for each of the repository's top, its common directory and its
//! object format. Here one `rev-parse` asks all three, once a process, and each answer is cut back
//! to what the single call prints. When the combined `rev-parse` fails (a bare repository, none at
//! all), or prints what cannot be cut apart with certainty, each fact is asked on its own when it
//! is needed, as Python asks it, so every failure answers exactly as before. These answers are the
//! environment's of the first question: a process asks in one, and none of them changes while it
//! runs.
//!
//! What can change while bead runs is asked when Python asks it, by Python's own call, and never
//! kept: origin's URLs each time the workspace is found ([`Checkout::origin_urls`], beside the
//! combined `rev-parse` the first time), the transport settings each time a link is made
//! ([`crate::transport_settings`]), and where the caller works, HEAD and its branch, each time a
//! transaction starts ([`head_and_branch`]): a `git config` or a `git checkout` while bead waits
//! for the store lock is what the command then goes by.

use std::sync::OnceLock;

use crate::error::Error;
use crate::runner::{Env, Output};

/// The keys that hold origin's URLs (`git config --get-regexp`).
pub const ORIGIN_KEYS: &str = r"^remote\.origin\.(push)?url$";

/// Python's question for origin's URLs, without `--null`.
const ORIGIN_ARGS: [&str; 3] = ["config", "--get-regexp", ORIGIN_KEYS];

/// One question about the checkout that does not change while bead runs, as Python asks it with
/// `git rev-parse`.
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

    /// The arguments Python gives git for this fact alone.
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
    /// What `git config --get-regexp` prints for [`ORIGIN_KEYS`] in `env`, asked now: Python asks
    /// it each time it finds the workspace. The first time, the combined `rev-parse` runs beside
    /// it: finding the workspace needs both, and neither waits for the other.
    pub fn origin_urls(&self, env: &Env) -> Result<Output, Error> {
        let origin = env.git(ORIGIN_ARGS);
        if self.combined.get().is_some() {
            return origin.run();
        }
        let (rev_parse, origin) = env.git(rev_parse_args()).run_alongside(origin);
        let _ = self.combined.set(combined_rev_parse(rev_parse));
        origin
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

/// What git answers when Python's `capture_where` asks where the caller works: `rev-parse HEAD`
/// and, only when that succeeded, `rev-parse --abbrev-ref HEAD`, each as its own call prints it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeadAnswers {
    /// `rev-parse HEAD`: the commit checked out.
    pub head: Output,
    /// `rev-parse --abbrev-ref HEAD`, asked once `head` succeeded: its branch, or `HEAD` when
    /// detached.
    pub branch: Option<Output>,
}

/// Python's two questions at once: HEAD's full name, then its branch.
const HEAD_ARGS: [&str; 4] = ["rev-parse", "HEAD", "--abbrev-ref", "HEAD"];

/// Where the caller works, asked of the checkout's git in `env` now, never kept: Python asks it
/// when each transaction starts, after the store lock is taken and again in each round of an
/// ownership command, and a `git checkout` in between is what an event's `where` records. One
/// `rev-parse HEAD --abbrev-ref HEAD` answers both when its two lines can only be what they stand
/// for: HEAD's full name (40 or 64 lower-case hex digits), then a branch. Anything else (no commit
/// yet, a branch name git finds ambiguous and prints nothing for, no repository) asks Python's two
/// calls, in its order.
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
    fn head_and_branch_answer_as_pythons_two_calls_each_time_they_are_asked() {
        let root = tempfile::tempdir().unwrap();
        for (name, env) in checkouts(root.path()) {
            let head = env.git(["rev-parse", "HEAD"]).run().unwrap();
            let branch = head.success().then(|| {
                env.git(["rev-parse", "--abbrev-ref", "HEAD"])
                    .run()
                    .unwrap()
            });
            let python = HeadAnswers { head, branch };
            assert_eq!(head_and_branch(&env).unwrap(), python, "{name}");
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
    fn origin_and_transport_settings_are_pythons_calls_asked_each_time() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path();
        let dir = home.join("w");
        std::fs::create_dir(&dir).unwrap();
        let env = crate::testing::clean_env(Some(&dir), home, &[]);
        git(&env, &["init", "-q"]);
        let transport = ["config", "--null", "--get-regexp", crate::TRANSPORT_KEYS];
        let checkout = Checkout::default();
        // the first question of a checkout without a commit yet: the combined rev-parse runs
        // beside origin's call and answers the facts
        let found = checkout.origin_urls(&env).unwrap();
        assert_eq!(found, env.git(ORIGIN_ARGS).run().unwrap());
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
        let found = checkout.origin_urls(&env).unwrap();
        assert_eq!(found, env.git(ORIGIN_ARGS).run().unwrap());
        assert_eq!(
            found.stdout,
            b"remote.origin.url ../o.git\nremote.origin.pushurl a b\nremote.origin.pushurl\n"
        );
        let settings = crate::transport_settings(&env).unwrap();
        assert_eq!(settings, env.git(transport).run().unwrap());
        assert_eq!(
            settings.stdout,
            b"remote.origin.pushurl\na b\0remote.origin.pushurl\0url.git@h:.insteadof\nh:\0"
        );
        // the push URLs are read from it, never carried into sync.git
        assert_eq!(
            crate::push_urls(&settings),
            [std::ffi::OsString::from("a b")]
        );
        let carried = crate::transport_config(&settings);
        assert!(
            carried.iter().all(|(key, _)| !key.ends_with(".pushurl")),
            "{carried:?}"
        );
    }

    #[test]
    fn each_fact_asks_what_python_asks() {
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
