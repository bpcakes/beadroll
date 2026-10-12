//! `V1_PushSideTranslation`: push-side race classification survives translated git (run-git area).
//! Each locale git has no translation for on this machine is skipped, and says so; the others still
//! run.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use beadroll_test_support::{Machine, Sandbox, parse_json};

use super::{
    Env, WINNER_IDENTITY, applied, bead_reads_rejected, contains, executable, git_is_translated,
    id, locales, member, quoted,
};

/// `bytes` as text, for a failure's message.
fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// ASCII whitespace, including vertical tab.
fn space(byte: u8) -> bool {
    byte.is_ascii_whitespace() || byte == 0x0b
}

/// `re.search(rb" ! \[rejected\] +\S+ -> refs/beads/journal \(non-fast-forward\)", stderr)`: git's
/// line for a push the remote refused as not a fast-forward.
fn names_the_lost_race(stderr: &[u8]) -> bool {
    const HEAD: &[u8] = b" ! [rejected]";
    const TAIL: &[u8] = b" -> refs/beads/journal (non-fast-forward)";
    (0..stderr.len()).any(|start| {
        let Some(rest) = stderr[start..].strip_prefix(HEAD) else {
            return false;
        };
        let spaces = rest.iter().take_while(|byte| **byte == b' ').count();
        let rest = &rest[spaces..];
        let word = rest.iter().take_while(|byte| !space(**byte)).count();
        spaces > 0 && word > 0 && rest[word..].starts_with(TAIL)
    })
}

/// The test's own `git`: `git ARGS` on the repository `git_dir` (`GIT_DIR`) under `env`, `data` on
/// its standard input, its status and output returned whatever they are.
fn git_in(env: &Env, git_dir: &Path, args: &[&str], data: Option<&[u8]>) -> Output {
    let mut command = Command::new("git");
    command
        .args(args)
        .env_clear()
        .envs(env)
        .env("GIT_DIR", git_dir)
        .current_dir(git_dir)
        .stdin(if data.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    if let Some(data) = data {
        let mut input = child.stdin.take().expect("git's standard input");
        input.write_all(data).unwrap();
    }
    child.wait_with_output().unwrap()
}

/// What `git_in` printed, as `.stdout.decode().strip()` reads it.
fn printed(output: &Output) -> String {
    String::from_utf8(output.stdout.clone())
        .unwrap()
        .trim()
        .to_owned()
}

/// The test's repositories: a bare client holding three commits of one tree, `c1`, `c2` (not a
/// descendant of `c1`) and `c3` (a child of `c1`), and a bare remote whose `refs/beads/journal` the
/// client pushes to; with `base`, the environment each of the test's git commands starts from.
struct Tokens {
    base: Env,
    remote: PathBuf,
    client: PathBuf,
    c1: String,
    c2: String,
    c3: String,
}

impl Tokens {
    /// The repositories in `sandbox`, the remote's journal at `c1`. `base` is the test's own
    /// environment without git's, locale or language variables, with an empty file for git's
    /// configuration and `t` making every commit.
    fn new(sandbox: &Sandbox) -> Tokens {
        let empty = sandbox.root().join("empty.gitconfig");
        std::fs::write(&empty, "").unwrap();
        let remote = sandbox.bare("tokens-remote.git");
        let client = sandbox.bare("tokens-client.git");
        let mut base: Env = std::env::vars_os()
            .filter(|(key, _)| {
                let key = key.as_encoded_bytes();
                !["GIT_", "LC_", "LANG"]
                    .iter()
                    .any(|prefix| key.starts_with(prefix.as_bytes()))
            })
            .collect();
        base.insert("GIT_CONFIG_GLOBAL".into(), empty.into());
        for (key, value) in [
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_AUTHOR_NAME", "t"),
            ("GIT_AUTHOR_EMAIL", "t@example.invalid"),
            ("GIT_COMMITTER_NAME", "t"),
            ("GIT_COMMITTER_EMAIL", "t@example.invalid"),
        ] {
            base.insert(key.into(), value.into());
        }
        let made =
            |args: &[&str], data: Option<&[u8]>| printed(&git_in(&base, &client, args, data));
        let blob = made(&["hash-object", "-w", "--stdin"], Some(b"x\n"));
        let entry = format!("100644 blob {blob}\tf\n");
        let tree = made(&["mktree"], Some(entry.as_bytes()));
        let c1 = made(&["commit-tree", &tree, "-m", "one"], None);
        let c2 = made(&["commit-tree", &tree, "-m", "two"], None);
        let c3 = made(&["commit-tree", &tree, "-p", &c1, "-m", "three"], None);
        let tokens = Tokens {
            base,
            remote,
            client,
            c1,
            c2,
            c3,
        };
        let first = tokens.push(&tokens.base, "-q", &tokens.c1);
        assert!(first.status.success(), "{}", lossy(&first.stderr));
        tokens
    }

    /// The client's `git push HOW REMOTE COMMIT:refs/beads/journal` under `env`.
    fn push(&self, env: &Env, how: &str, commit: &str) -> Output {
        let to = self.remote.to_str().expect("a UTF-8 path");
        let refspec = format!("{commit}:refs/beads/journal");
        git_in(env, &self.client, &["push", how, to, &refspec], None)
    }

    /// The remote's journal set back to `c1`, where each locale's cases start.
    fn reset(&self) {
        let args = ["update-ref", "refs/beads/journal", &self.c1];
        git_in(&self.base, &self.remote, &args, None);
    }

    /// The client-side lost race, under the locale `name` (`env`): `c2` refused as not a fast
    /// forward, in English words bead reads, the porcelain line too, and only git's summary
    /// translated.
    fn lost_race(&self, name: &str, env: &Env) {
        let lost = self.push(env, "--quiet", &self.c2);
        let said = lossy(&lost.stderr);
        assert!(!lost.status.success(), "{name}: {said}");
        assert!(names_the_lost_race(&lost.stderr), "{name}: {said}");
        assert!(bead_reads_rejected(&lost.stderr), "{name}: {said}");
        let porcelain = self.push(env, "--porcelain", &self.c2);
        let line = format!(
            "!\t{}:refs/beads/journal\t[rejected] (non-fast-forward)",
            self.c2
        );
        assert!(
            contains(&porcelain.stdout, line.as_bytes()),
            "{name}: {}",
            lossy(&porcelain.stdout)
        );
        if name == "C" {
            let english = contains(&lost.stderr, b"error: failed to push some refs");
            assert!(english, "{name}: {said}");
        } else {
            // the locale took effect
            let english = contains(&lost.stderr, b"failed to push some refs");
            assert!(!english, "{name}: {said}");
        }
    }

    /// The policy refusal of the pre-receive hook the caller wrote, under the locale `name`
    /// (`env`): `c3` declined, in English words bead reads.
    fn refusal(&self, name: &str, env: &Env) {
        let refused = self.push(env, "--quiet", &self.c3);
        let said = lossy(&refused.stderr);
        assert!(!refused.status.success(), "{name}: {said}");
        let rejected = contains(&refused.stderr, b"[remote rejected]");
        assert!(rejected, "{name}: {said}");
        let declined = contains(&refused.stderr, b"(pre-receive hook declined)");
        assert!(declined, "{name}: {said}");
        assert!(bead_reads_rejected(&refused.stderr), "{name}: {said}");
    }

    /// The server-side lost race the pre-receive hook the caller wrote stages, under the locale
    /// `name` (`env`): the ref moved while `c3` was received, in English words bead reads.
    fn server_race(&self, name: &str, env: &Env) {
        let raced = self.push(env, "--quiet", &self.c3);
        let said = lossy(&raced.stderr);
        assert!(!raced.status.success(), "{name}: {said}");
        let locked = b"remote: error: cannot lock ref 'refs/beads/journal'";
        assert!(contains(&raced.stderr, locked), "{name}: {said}");
        let rejected = contains(&raced.stderr, b"[remote rejected]");
        assert!(rejected, "{name}: {said}");
        let stale = contains(&raced.stderr, b"(incorrect old value provided)");
        assert!(stale, "{name}: {said}");
        assert!(bead_reads_rejected(&raced.stderr), "{name}: {said}");
    }
}

/// Verified (V1): the push-side tokens Link.publish matches stay English under translated git, so
/// a lost race is still read as 'rejected'. The review compared pure git across 6 locale settings
/// (C, LANG=de_DE.UTF-8, LC_ALL=de_DE.UTF-8, LANG=fr_FR.UTF-8, LANG=fr_FR.ISO8859-1, LANGUAGE=de
/// with LANG=en_US.UTF-8): in all 6, ' ! [rejected] ... (non-fast-forward)', ' ! [remote rejected]
/// ... (pre-receive hook declined)', 'remote: error: cannot lock ref ...' and ' ! [remote rejected]
/// ... (incorrect old value provided)' stay English, and the porcelain status line
/// '!\t<sha>:refs/beads/journal\t[rejected] (non-fast-forward)' is untranslated; only 'error:
/// failed to push some refs' and the hints are translated ('Fehler: Fehler beim Versenden...',
/// 'erreur : impossible de pousser...'). Locales without git translations skip.
#[test]
fn git_keeps_the_rejection_tokens_bead_reads_untranslated() {
    let sandbox = crate::sandbox();
    let tokens = Tokens::new(&sandbox);
    let hook = tokens.remote.join("hooks").join("pre-receive");
    let refuse = "#!/bin/sh\ncat >/dev/null\necho 'policy: refs/beads/* are read-only here' \
                  >&2\nexit 1\n";
    let race = format!(
        "#!/bin/sh\ncat >/dev/null\nunset GIT_QUARANTINE_PATH GIT_OBJECT_DIRECTORY \
         GIT_ALTERNATE_OBJECT_DIRECTORIES\n{WINNER_IDENTITY}W=$(GIT_DIR={r} git rev-parse \
         refs/beads/journal)\nN=$(GIT_DIR={r} git commit-tree $W^{{tree}} -p $W -m winner)\n\
         GIT_DIR={r} git update-ref refs/beads/journal $N $W\nexit 0\n",
        r = quoted(&tokens.remote)
    );
    for (name, setting) in locales() {
        let env = applied(&tokens.base, &setting);
        if name != "C" && !git_is_translated(&env, sandbox.root()) {
            eprintln!("skipped {name}: git has no translation for {name} on this machine");
            continue;
        }
        if let Err(error) = std::fs::remove_file(&hook) {
            assert_eq!(error.kind(), std::io::ErrorKind::NotFound, "{error}");
        }
        tokens.reset();
        // 1. client-side lost race: the remote head is not an ancestor of what we push
        tokens.lost_race(name, &env);
        // 2. a policy refusal by pre-receive
        executable(&hook, refuse);
        tokens.refusal(name, &env);
        // 3. server-side lost race: a concurrent winner moves the ref while ours is received
        std::fs::write(&hook, &race).unwrap();
        tokens.server_race(name, &env);
    }
}

/// Verified (V1): under translated git a coordinated publication that loses a race is still
/// recognised as rejected and retried. The review ran bead end to end under LANG=de_DE.UTF-8: a
/// server-side race inside one sync (a pre-receive moves the ref once, so git reports 'cannot lock
/// ref') gave sync rc=0 with attempts=2; its TestCase passed 3 subtests (LANG=de_DE.UTF-8,
/// LANG=fr_FR.UTF-8, LANGUAGE=de with LANG=en_US.UTF-8) on e9eed92. The first publication runs with
/// LC_ALL=C, because the fetch side of a never-published remote is translated (lead observation
/// 1). Locales without git translations skip.
#[test]
fn lost_races_are_recognised_and_retried_under_translated_git() {
    let measured = [
        "LANG=de_DE.UTF-8",
        "LANG=fr_FR.UTF-8",
        "LANGUAGE=de LANG=en_US.UTF-8",
    ];
    let chosen = locales()
        .into_iter()
        .filter(|(name, _)| measured.contains(name));
    for (name, setting) in chosen {
        let sandbox = crate::sandbox();
        let mut m = Machine::new(&sandbox, "x");
        for (key, value) in &setting {
            m.extra.insert(key.into(), value.into());
        }
        if !git_is_translated(&m.env(None, "codex"), sandbox.root()) {
            eprintln!("skipped {name}: git has no translation for {name} on this machine");
            continue;
        }
        m.bead(["init", "--prefix", "ex"]);
        let key = id(m.bead(["create", "one"]));
        // first publication
        let first = m.cmd(["sync", "--json"]).env("LC_ALL", "C").run();
        assert_eq!(first.returncode(), 0, "{name}: {}", first.stderr);
        let gate = sandbox.root().join("raced");
        // a concurrent winner moves the ref, once
        let hook = sandbox.origin().join("hooks").join("pre-receive");
        let script = format!(
            "#!/bin/sh\ncat >/dev/null\nunset GIT_QUARANTINE_PATH GIT_OBJECT_DIRECTORY \
             GIT_ALTERNATE_OBJECT_DIRECTORIES\n{WINNER_IDENTITY}if mkdir {gate} 2>/dev/null; \
             then\n  W=$(GIT_DIR={origin} git rev-parse refs/beads/journal)\n  \
             N=$(GIT_DIR={origin} git commit-tree $W^{{tree}} -p $W -m winner)\n  \
             GIT_DIR={origin} git update-ref refs/beads/journal $N $W\nfi\nexit 0\n",
            gate = quoted(&gate),
            origin = quoted(sandbox.origin()),
        );
        executable(&hook, &script);
        // the sync below is the racing push
        m.bead(["comments", "add", &key, "racing", "--offline"]);
        let synced = m.run(["sync", "--json"]);
        assert_eq!(synced.returncode(), 0, "{name}: {}", synced.stderr);
        assert!(gate.exists(), "{name}");
        let result = parse_json(&synced.stdout).unwrap();
        assert_eq!(
            member(&result, "attempts").as_i64(),
            Some(2),
            "{name}: {result:?}"
        );
    }
}
