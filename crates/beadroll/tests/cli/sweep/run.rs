//! One schedule: a sandbox, three machines, a read-only remote observer, their shared tracker,
//! the commands and all violations found.

use std::collections::BTreeMap;
use std::fs::Permissions;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::Command;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use beadroll_core::json::{Format, write};
use beadroll_test_support::{Json, Machine, Sandbox};

use super::call::{Step, answer, exec, tail};
use super::{
    A1, A2, B1, C1, Keys, OPS, Op, Pre, STALL, STALL_FENCED, STALL_LANDED, Schedule, Variant,
    fault_env, fault_fired,
};

mod observe;
mod settle;

/// The observer's thread.
pub(super) const OBS: &str = "obs-thread";

/// X, Y and Z as the observer read them from the remote, by letter; empty when it could not.
pub(super) type Remote = BTreeMap<char, Json>;

/// What the follow-ups of the concurrent variant may rest on: the remote, and its export, before
/// them.
pub(super) struct Earlier {
    pub(super) remote: Remote,
    pub(super) export: String,
}

/// One schedule and what it found.
pub(super) struct Run {
    pub(super) schedule: Schedule,
    pub(super) variant: Variant,
    sandbox: Sandbox,
    m1: Machine,
    m2: Machine,
    m3: Machine,
    pub(super) obs: Machine,
    pub(super) keys: Keys,
    /// Each machine's identity, by machine name, as `bead info` gives it.
    replica: BTreeMap<String, String>,
    steps: Mutex<Vec<Step>>,
    violations: Mutex<Vec<String>>,
    /// The generations of X's claim each machine has shown, in order.
    generations: Mutex<BTreeMap<String, Vec<i64>>>,
    /// The syncs that reported forked journals, with what they reported.
    forks: Mutex<Vec<(String, Json)>>,
}

/// `mutex`'s value, also after a panic of the schedule's driver while it held it.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `value` as one line of JSON, for a message.
pub(super) fn shown(value: &Json) -> String {
    write(value, Format::SPACED).unwrap_or_else(|_| format!("{value:?}"))
}

/// Installs the hook `name` of the sandbox's remote, `script`.
fn hook(sandbox: &Sandbox, name: &str, script: &str) {
    let path = sandbox.origin().join("hooks").join(name);
    std::fs::write(&path, script).expect("the remote's hook");
    std::fs::set_permissions(&path, Permissions::from_mode(0o755)).expect("an executable hook");
}

/// `path` quoted for the shell, as `shlex.quote` quotes it: as it is when every character is safe.
fn shell_quoted(path: &Path) -> String {
    let text = path.to_str().expect("a UTF-8 sandbox path");
    let safe = |c: char| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c);
    if !text.is_empty() && text.chars().all(safe) {
        text.to_owned()
    } else {
        format!("'{}'", text.replace('\'', r#"'"'"'"#))
    }
}

impl Run {
    /// A new sandbox with machines m1, m2, m3 and the observer, each running the `bead` Cargo
    /// built. With `gc`, the remote collects garbage after every push it accepts, synchronously.
    pub(super) fn new(schedule: Schedule, variant: Variant) -> Run {
        let sandbox = crate::sandbox();
        if variant.gc {
            let script = "#!/bin/sh\ngit gc --prune=now --quiet >/dev/null 2>&1\nexit 0\n";
            hook(&sandbox, "post-receive", script);
        }
        let [m1, m2, m3, obs] =
            ["m1", "m2", "m3", "obs"].map(|name| crate::rust_only(&sandbox, name));
        let generations = ["m1", "m2", "m3"].map(|name| (name.to_owned(), Vec::new()));
        Run {
            schedule,
            variant,
            sandbox,
            m1,
            m2,
            m3,
            obs,
            keys: Keys::default(),
            replica: BTreeMap::new(),
            steps: Mutex::new(Vec::new()),
            violations: Mutex::new(Vec::new()),
            generations: Mutex::new(BTreeMap::from(generations)),
            forks: Mutex::new(Vec::new()),
        }
    }

    // -- running commands

    /// Runs `bead ARGV --json` on `machine` as `thread`, with `env` set last, and records the step
    /// under `label`. A run that never ends, or crashes, is a violation.
    pub(super) fn call<S: AsRef<str>>(
        &self,
        machine: &Machine,
        argv: &[S],
        thread: &str,
        env: Option<(&str, &str)>,
        label: &str,
    ) -> Step {
        let argv: Vec<String> = argv.iter().map(|word| word.as_ref().to_owned()).collect();
        let mut invocation = machine
            .cmd(argv.iter().map(String::as_str).chain(["--json"]))
            .thread(thread);
        if let Some((key, value)) = env {
            invocation = invocation.env(key, value);
        }
        let ran = exec(invocation);
        let (out, code) = answer(&ran);
        let crashed = ran.crashed(code.as_deref());
        let step = Step {
            step: label.to_owned(),
            machine: machine.name().to_owned(),
            thread: thread.to_owned(),
            argv,
            rc: ran.rc,
            code,
            ms: ran.ms,
            out,
            raw: ran.stdout,
            err: tail(&ran.stderr, 400).to_owned(),
        };
        if crashed {
            let said = tail(&ran.stderr, 300);
            self.violation(format!("{}: {} {said}", step.step, step.rc));
        }
        if step.argv[0] == "sync" {
            let forked = step.out.as_ref().map(|out| &out["forked_journals"]);
            if let Some(forked) = forked.filter(|forked| forked.truthy()) {
                lock(&self.forks).push((step.step.clone(), forked.clone()));
            }
        }
        lock(&self.steps).push(step.clone());
        step
    }

    /// A step of the setup, which must succeed: what it printed.
    fn ok(&self, machine: &Machine, argv: &[&str], thread: &str) -> Option<Json> {
        let step = self.call(machine, argv, thread, None, "setup");
        assert!(step.rc.is(0), "setup {argv:?} failed: {}", step.err);
        step.out
    }

    /// The id of the issue a setup step created.
    fn created(&self, argv: &[&str]) -> String {
        let out = self
            .ok(&self.m1, argv, A1)
            .expect("create prints the issue");
        out["id"].as_str().expect("the new issue's id").to_owned()
    }

    /// `bead export` on `machine`, on its default thread, without `--json`, which must succeed.
    fn export(machine: &Machine) -> String {
        let ran = exec(machine.cmd(["export"]));
        assert!(ran.rc.is(0), "bead export failed: {}", ran.stderr);
        ran.stdout
    }

    /// Records a violation.
    pub(super) fn violation(&self, text: String) {
        lock(&self.violations).push(text);
    }

    /// The machine named `name`.
    pub(super) fn machine(&self, name: &str) -> &Machine {
        match name {
            "m1" => &self.m1,
            "m2" => &self.m2,
            "m3" => &self.m3,
            _ => panic!("no machine {name}"),
        }
    }

    /// The three machines, each with the thread that works there.
    fn machines(&self) -> [(&Machine, &'static str); 3] {
        [(&self.m1, A1), (&self.m2, B1), (&self.m3, C1)]
    }

    // -- setup

    /// The tracker: Q, X (Q's child), Y, Z and P, on every machine; then the operation's
    /// precondition.
    fn prepare(&mut self) {
        let (m1, m2, m3) = (&self.m1, &self.m2, &self.m3);
        self.ok(m1, &["init", "--prefix", "sw"], A1);
        let q = self.created(&["create", "old parent q"]);
        let x = self.created(&["create", "target x", "--parent", &q]);
        let mut keys = BTreeMap::from([('Q', q), ('X', x)]);
        for name in ['Y', 'Z', 'P'] {
            let title = format!("issue {}", name.to_ascii_lowercase());
            keys.insert(name, self.created(&["create", &title]));
        }
        for (machine, thread) in self.machines() {
            self.ok(machine, &["sync"], thread);
        }
        // A machine that has not written yet has no identity.
        let replica = [m1, m2, m3].map(|machine| {
            let info = self
                .ok(machine, &["info"], A1)
                .expect("info prints a record");
            let id = info["machine"].as_str().unwrap_or("None");
            (machine.name().to_owned(), id.to_owned())
        });
        let keys = Keys(keys);
        let (x, y) = (keys.of('X'), keys.of('Y'));
        match self.op().pre {
            Pre::Free => {}
            Pre::A1Holds => {
                self.ok(m1, &["claim", x], A1);
                self.ok(m2, &["sync"], B1);
                self.ok(m3, &["sync"], C1);
            }
            Pre::B1Holds => {
                self.ok(m2, &["claim", x], B1);
                self.ok(m1, &["sync"], A1);
                self.ok(m3, &["sync"], C1);
            }
            Pre::Sync => {
                // a comment publishes at once; offline it waits for the sync under test
                self.ok(m1, &["comments", "add", x, "local work", "--offline"], A1);
                self.ok(m2, &["claim", y], B1);
            }
        }
        self.keys = keys;
        self.replica = BTreeMap::from(replica);
    }

    /// The operation of the schedule.
    fn op(&self) -> &'static Op {
        OPS.iter()
            .find(|op| op.name == self.schedule.op)
            .unwrap_or_else(|| panic!("no operation {}", self.schedule.op))
    }

    /// The owner label `thread` on the machine `machine` has: `codex:THREAD@REPLICA`.
    pub(super) fn label(&self, machine: &str, thread: &str) -> String {
        format!("codex:{thread}@{}", self.replica[machine])
    }

    // -- faults

    /// The variable that injects the schedule's fault into the operation's run; a timeout installs
    /// the remote's stall hook first.
    fn fault_args(&self) -> (&'static str, &'static str) {
        let fault = self.schedule.fault;
        if fault.starts_with("timeout") {
            let template = if fault == "timeout-fenced" {
                STALL_FENCED
            } else {
                STALL_LANDED
            };
            let root = shell_quoted(self.sandbox.root());
            let script = template.replace("{root}", &root).replace("{stall}", STALL);
            hook(&self.sandbox, "pre-receive", &script);
        }
        fault_env(self.schedule.op, fault)
    }

    /// A violation unless the operation's run `op` shows that the schedule's fault fired
    /// ([`fault_fired`]): the stall hook's mark stays after the hook is taken away.
    fn fired(&self, op: &Step) {
        let stalled = self.sandbox.root().join("hook-started").exists();
        if !fault_fired(self.schedule.fault, op.rc, stalled) {
            let code = op.code.as_deref().unwrap_or("-");
            self.violation(format!(
                "op: {} never fired: bead {} -> rc {} {code} ({} ms)",
                self.schedule.fault,
                op.argv.join(" "),
                op.rc,
                op.ms
            ));
        }
    }

    /// Lets a stalled receive-pack finish (landing or failing), then takes the stall hook away.
    fn quiesce(&self) {
        let hook = self.sandbox.origin().join("hooks").join("pre-receive");
        if !hook.exists() {
            return;
        }
        let root = self.sandbox.root();
        let deadline = Instant::now() + Duration::from_secs(20);
        if root.join("hook-started").exists() {
            while !root.join("hook-finished").exists() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        let pattern = format!("receive-pack.*{}", self.sandbox.origin().display());
        loop {
            if Instant::now() >= deadline {
                let text = "a receive-pack was still running 20 s after the stalled push";
                self.violation(text.to_owned());
                break;
            }
            let busy = Command::new("pgrep")
                .args(["-f", &pattern])
                .output()
                .expect("pgrep runs");
            if String::from_utf8_lossy(&busy.stdout).trim().is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        std::fs::remove_file(&hook).expect("the stall hook goes");
    }

    // -- the schedule

    /// Runs the follow-up `letter` after the operation `op_argv`: the step, and who ran it.
    fn follow(&self, letter: char, op_argv: &[String]) -> (Step, (&'static str, &'static str)) {
        let (x, z) = (self.keys.of('X'), self.keys.of('Z'));
        match letter {
            'R' => (self.call(&self.m1, op_argv, A1, None, "R"), ("m1", A1)),
            'D' => (
                self.call(&self.m1, &["claim", z], A1, None, "D"),
                ("m1", A1),
            ),
            'O' => (
                self.call(&self.m1, &["claim", x], A2, None, "O"),
                ("m1", A2),
            ),
            _ => (self.call(&self.m2, &["sync"], B1, None, "S"), ("m2", B1)),
        }
    }

    /// The schedule: the tracker, the operation stopped at its fault, the remote left to settle,
    /// the two follow-ups (one after the other, or at once), and each answer judged against the
    /// remote right after it; then every machine settled and compared.
    pub(super) fn execute(&mut self) {
        self.prepare();
        let run: &Run = self;
        let op_argv = (run.op().argv)(&run.keys);
        let env = run.fault_args();
        let first = run.call(&run.m1, &op_argv, A1, Some(env), "op");
        run.quiesce();
        run.fired(&first);
        let before = run.observe("op");
        run.judge(&first, &before, ("m1", A1), None);
        let pair = run.schedule.pair;
        if run.variant.concurrent {
            let export = run
                .call(&run.obs, &["export"], OBS, None, "observer export")
                .raw;
            let earlier = Earlier {
                remote: before,
                export,
            };
            let op_argv = &op_argv;
            let done = std::thread::scope(|scope| {
                let followers: Vec<_> = pair
                    .chars()
                    .map(|letter| scope.spawn(move || run.follow(letter, op_argv)))
                    .collect();
                let joined = followers.into_iter().map(|follower| follower.join());
                let done: Vec<_> = joined
                    .map(|done| done.unwrap_or_else(|panic| std::panic::resume_unwind(panic)))
                    .collect();
                done
            });
            let letters: Vec<String> = pair.chars().map(String::from).collect();
            let remote = run.observe(&letters.join("+"));
            for (step, who) in done {
                run.judge(&step, &remote, who, Some(&earlier));
            }
        } else {
            for letter in pair.chars() {
                let (step, who) = run.follow(letter, &op_argv);
                let remote = run.observe(&letter.to_string());
                run.judge(&step, &remote, who, None);
            }
        }
        run.settle();
    }

    /// The schedule's result: its violations, and its steps but those of the setup and the views.
    pub(super) fn report(&self) -> (Vec<String>, Vec<Step>) {
        let steps = lock(&self.steps)
            .iter()
            .filter(|step| {
                let looked = step.step.starts_with("view") || step.step.starts_with("observer");
                step.step != "setup" && !looked
            })
            .cloned()
            .collect();
        (lock(&self.violations).clone(), steps)
    }
}
