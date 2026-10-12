//! Machines: one developer machine each, with its own home, clone and identity.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::io::Write as _;
use std::os::unix::process::ExitStatusExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

use crate::{Json, Sandbox};

/// Environment prefixes removed so the developer's harness, store, repository and colour settings
/// cannot reach the binary under test.
const SCRUBBED: [&str; 6] = ["CLAUDE", "CODEX", "BEAD_", "GIT_", "NO_COLOR", "CLICOLOR"];

/// One simulated developer machine with its own BEAD_HOME, clone and identity.
pub struct Machine {
    name: String,
    home: PathBuf,
    checkout: PathBuf,
    bead: PathBuf,
    /// Variables every run sets after the identity, as `Machine.extra` (`BEAD_ROTATE_BYTES`, say).
    pub extra: BTreeMap<OsString, OsString>,
}

impl Machine {
    /// A machine `name` in `sandbox`: home `home-NAME` (bead creates it), checkout `NAME-main`
    /// cloned from `origin.git`, running the sandbox's `bead`.
    pub fn new(sandbox: &Sandbox, name: &str) -> Machine {
        Machine {
            name: name.to_owned(),
            home: sandbox.root().join(format!("home-{name}")),
            checkout: sandbox.clone(&format!("{name}-main"), None),
            bead: sandbox.bead().to_path_buf(),
            extra: BTreeMap::new(),
        }
    }

    /// This machine runs `bead` whatever the sandbox chose, `BEAD_UNDER_TEST` included: one
    /// binary per machine, for a test that needs this build's own (its failpoints) or a stand-in.
    /// It must be an absolute path.
    pub fn with_bead(mut self, bead: impl Into<PathBuf>) -> Machine {
        let bead = bead.into();
        assert!(
            bead.is_absolute(),
            "a machine's bead must be an absolute path, not {}",
            bead.display()
        );
        self.bead = bead;
        self
    }

    /// The machine's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The machine's `BEAD_HOME`.
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// The machine's clone, where its commands run unless told otherwise.
    pub fn checkout(&self) -> &Path {
        &self.checkout
    }

    /// The `bead` this machine runs.
    pub fn executable(&self) -> &Path {
        &self.bead
    }

    /// The inherited environment without CLAUDE*, CODEX*, BEAD_*, GIT_*, NO_COLOR or CLICOLOR*
    /// variables (whether bead colours its output is each test's to say), followed by
    /// this machine's home and identity and then Machine::extra. The default thread is
    /// NAME-thread-0001.
    pub fn env(&self, thread: Option<&str>, agent: &str) -> BTreeMap<OsString, OsString> {
        let mut env = scrubbed(std::env::vars_os());
        let thread = thread.map_or_else(|| format!("{}-thread-0001", self.name), str::to_owned);
        env.insert("BEAD_HOME".into(), self.home.clone().into_os_string());
        env.insert("BEAD_AGENT".into(), agent.into());
        env.insert("BEAD_THREAD".into(), thread.into());
        env.extend(self.extra.clone());
        env
    }

    /// `bead ARGS` from this machine, to adjust before running it.
    pub fn cmd<I, S>(&self, args: I) -> Invocation<'_>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        Invocation {
            machine: self,
            args: args.into_iter().map(|a| a.as_ref().to_owned()).collect(),
            cwd: None,
            thread: None,
            agent: "codex".to_owned(),
            more: BTreeMap::new(),
            stdin: None,
        }
    }

    /// Runs `bead ARGS` as `Machine.run` does: in the checkout, as agent `codex` on the machine's
    /// default thread.
    pub fn run<I, S>(&self, args: I) -> Run
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.cmd(args).run()
    }

    /// Runs `bead ARGS --json`: the parsed document, or `None` when the
    /// output is empty; a failure panics with bead's standard error.
    pub fn bead<I, S>(&self, args: I) -> Option<Json>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.cmd(args).json()
    }

    /// `bead export`'s output; a failure panics.
    pub fn export(&self) -> String {
        let run = self.run(["export"]);
        assert!(run.status.success(), "bead export failed: {}", run.stderr);
        run.stdout
    }

    /// The issue `key`, as `bead show KEY --json` gives it.
    pub fn show(&self, key: &str) -> Json {
        let shown = self.bead(["show", key]).expect("bead show prints a list");
        shown[0].clone()
    }

    /// The ids `bead ready` offers.
    pub fn ready(&self) -> BTreeSet<String> {
        let ready = self.bead(["ready"]).expect("bead ready prints a list");
        let issues = ready.as_array().expect("bead ready prints a list");
        issues
            .iter()
            .map(|issue| issue["id"].as_str().expect("an id").to_owned())
            .collect()
    }

    /// The workspace store in the machine's home: its first directory, as `Machine.store` takes it.
    pub fn store(&self) -> PathBuf {
        let entries = std::fs::read_dir(&self.home).expect("the machine's home");
        entries
            .map(|entry| entry.expect("an entry of the home").path())
            .find(|path| path.is_dir())
            .expect("a store in the machine's home")
    }
}

/// Inherited variables remaining after harness, tracker and git settings are removed.
fn scrubbed(vars: impl Iterator<Item = (OsString, OsString)>) -> BTreeMap<OsString, OsString> {
    vars.filter(|(key, _)| {
        let key = key.as_encoded_bytes();
        !SCRUBBED
            .iter()
            .any(|prefix| key.starts_with(prefix.as_bytes()))
    })
    .collect()
}

/// One `bead` invocation from a machine, shaped as `Machine.run`'s keyword arguments shape it.
#[must_use = "an invocation does nothing until it runs"]
pub struct Invocation<'m> {
    machine: &'m Machine,
    args: Vec<OsString>,
    cwd: Option<PathBuf>,
    thread: Option<String>,
    agent: String,
    more: BTreeMap<OsString, OsString>,
    stdin: Option<Vec<u8>>,
}

impl Invocation<'_> {
    /// Runs in `dir` instead of the machine's checkout.
    pub fn cwd(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cwd = Some(dir.into());
        self
    }

    /// Runs on `thread` instead of the machine's default thread.
    pub fn thread(mut self, thread: impl Into<String>) -> Self {
        self.thread = Some(thread.into());
        self
    }

    /// Runs as `agent` instead of `codex`.
    pub fn agent(mut self, agent: impl Into<String>) -> Self {
        self.agent = agent.into();
        self
    }

    /// Sets `key` last, over the identity and [`Machine::extra`].
    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.more.insert(key.into(), value.into());
        self
    }

    /// Feeds `input` to standard input, which is otherwise empty.
    pub fn stdin(mut self, input: impl Into<Vec<u8>>) -> Self {
        self.stdin = Some(input.into());
        self
    }

    /// The command this invocation runs, for a test that starts and waits for it itself.
    pub fn command(&self) -> Command {
        let mut env = self.machine.env(self.thread.as_deref(), &self.agent);
        env.extend(self.more.clone());
        let mut command = Command::new(&self.machine.bead);
        command
            .args(&self.args)
            .current_dir(self.cwd.as_deref().unwrap_or(&self.machine.checkout))
            .env_clear()
            .envs(env);
        command
    }

    /// Runs it to the end and returns what it printed.
    pub fn run(self) -> Run {
        let mut command = self.command();
        command
            .stdin(if self.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .unwrap_or_else(|error| panic!("cannot start {:?}: {error}", command.get_program()));
        // Written from another thread: a child that fills its output pipes before reading all of
        // its input would otherwise wait for us while we wait for it.
        let feeder = child.stdin.take().zip(self.stdin).map(|(mut pipe, input)| {
            std::thread::spawn(move || {
                let _ = pipe.write_all(&input);
            })
        });
        let output = child.wait_with_output().expect("bead's output");
        if let Some(feeder) = feeder {
            feeder.join().expect("the stdin writer");
        }
        Run {
            status: output.status,
            stdout: text(output.stdout, "standard output"),
            stderr: text(output.stderr, "standard error"),
        }
    }

    /// Runs it with `--json`: the parsed document, or `None` when the output
    /// is empty; a failure panics with bead's standard error.
    pub fn json(mut self) -> Option<Json> {
        let shown = self
            .args
            .iter()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
        self.args.push("--json".into());
        let run = self.run();
        assert!(run.status.success(), "bead {shown} failed: {}", run.stderr);
        if run.stdout.trim().is_empty() {
            return None;
        }
        let document = crate::parse_json(&run.stdout).unwrap_or_else(|error| {
            panic!("bead {shown} printed no JSON ({error}): {}", run.stdout)
        });
        Some(document)
    }
}

/// Output decoded as UTF-8, or the test fails.
fn text(bytes: Vec<u8>, stream: &str) -> String {
    String::from_utf8(bytes).unwrap_or_else(|error| panic!("bead's {stream} is not UTF-8: {error}"))
}

/// A finished run of `bead`.
#[derive(Debug)]
pub struct Run {
    /// How the process ended.
    pub status: ExitStatus,
    /// Its standard output.
    pub stdout: String,
    /// Its standard error.
    pub stderr: String,
}

impl Run {
    /// The exit code, or the negated terminating signal number.
    pub fn returncode(&self) -> i32 {
        match (self.status.code(), self.status.signal()) {
            (Some(code), _) => code,
            (None, Some(signal)) => -signal,
            (None, None) => panic!("{:?} has neither an exit code nor a signal", self.status),
        }
    }

    /// Whether bead crashed instead of answering: traceback, panic (`panicked at` or exit 101),
    /// SIGABRT (6) or SIGSEGV (11). Applies to the default binary and alternate executables.
    pub fn crashed(&self) -> bool {
        self.stderr.contains("Traceback")
            || self.stderr.contains("panicked at")
            || matches!(self.returncode(), 101 | -6 | -11)
    }
}

/// The structured error code reported on stderr by a failed JSON command.
pub fn code(run: &Run) -> String {
    let document = crate::parse_json(&run.stderr)
        .unwrap_or_else(|error| panic!("bead's error is not JSON ({error}): {}", run.stderr));
    let code = document["error"]["code"].as_str();
    code.unwrap_or_else(|| panic!("no error code in {}", run.stderr))
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(names: &[&str]) -> impl Iterator<Item = (OsString, OsString)> {
        names
            .iter()
            .map(|name| (OsString::from(*name), OsString::from("x")))
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn the_scrub_drops_harness_store_and_repository_variables() {
        let kept = scrubbed(vars(&[
            "CLAUDE_CODE_SESSION_ID",
            "CLAUDECODE",
            "CODEX_THREAD_ID",
            "BEAD_UNDER_TEST",
            "BEAD_HOME",
            "GIT_DIR",
            "GIT_CONFIG_GLOBAL",
            "PATH",
            "HOME",
            "BEADROLL_ORACLE_REFUSALS",
            "LC_ALL",
        ]));
        let names: Vec<_> = kept.keys().map(|k| k.to_str().unwrap()).collect();
        assert_eq!(
            names,
            ["BEADROLL_ORACLE_REFUSALS", "HOME", "LC_ALL", "PATH"]
        );
    }
}
