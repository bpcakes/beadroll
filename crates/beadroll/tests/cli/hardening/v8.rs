//! V8: terminal output matches pipe output, transport cannot prompt, and hangup ends bead
//! quietly. Remote and local git maintenance, including repair of a lost remote tip, must
//! preserve convergence.
//!
//! External tools create terminals without unsafe test code. `script` makes its child session
//! leader and copies terminal output to stdout; the hangup tests document the additional setup
//! they need. Platform coverage is checked by xtask's CI tests.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, ExitStatus};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use beadroll_test_support::{Machine, json, parse_json};

mod v8_hangup_of_the_controlling_terminal;
mod v8_prune_on_the_private_sync_repo;
mod v8_pty_output_matches_pipe_output;
mod v8_recovery_from_a_pruned_remote;
mod v8_remote_maintenance;
mod v8_sync_under_a_controlling_terminal_cannot_prompt;

/// The claims a machine lists, as (id, owner, state), sorted.
type Claims = Vec<(String, String, String)>;

/// The claims `machine` lists: `sorted((c["id"], c["owner"], c["state"]) for c in claims)`.
fn claims_of(machine: &Machine) -> Claims {
    let listed = machine
        .bead(["claims"])
        .expect("bead claims prints a record");
    let rows = listed["claims"].as_array().expect("a list of claims");
    let text = |value: &beadroll_test_support::Json| value.as_str().expect("a string").to_owned();
    let mut claims: Claims = rows
        .iter()
        .map(|c| (text(&c["id"]), text(&c["owner"]), text(&c["state"])))
        .collect();
    claims.sort();
    claims
}

/// `_v8_converge_and_compare`: two full sync rounds, then identical exports and claims, a clean
/// `info` and a clean remote fsck; the claims each machine lists, by machine name.
fn converge_and_compare(machines: &[&Machine], remote: &Path) -> BTreeMap<String, Claims> {
    for _ in 0..2 {
        for machine in machines {
            machine.bead(["sync"]);
        }
    }
    let exports: BTreeSet<String> = machines.iter().map(|m| m.export()).collect();
    let claims: BTreeMap<String, Claims> = machines
        .iter()
        .map(|m| (m.name().to_owned(), claims_of(m)))
        .collect();
    assert_eq!(exports.len(), 1, "exports differ between machines");
    let first = machines[0].name();
    for (name, seen) in &claims {
        assert_eq!(seen, &claims[first], "claims on {name} differ from {first}");
    }
    for machine in machines {
        let info = machine.bead(["info"]).expect("info");
        assert_eq!(info["integrity"], json!([]), "{}", machine.name());
        assert_eq!(
            info["waiting_events"].as_i64(),
            Some(0),
            "{}",
            machine.name()
        );
        assert_eq!(info["pending_ownership"], json!([]), "{}", machine.name());
    }
    let fsck = Command::new("git")
        .arg("--git-dir")
        .arg(remote)
        .args(["fsck", "--full", "--strict"])
        .output()
        .expect("git starts");
    assert!(
        fsck.status.success(),
        "{}",
        String::from_utf8_lossy(&fsck.stderr)
    );
    claims
}

/// `_v8_rng`: a small deterministic generator in [0, 1) (a 64-bit LCG), so the operation mix is
/// reproducible.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }

    /// `_v8_pick`: one of `items`.
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        let index = (self.next() * items.len() as f64) as usize;
        &items[index.min(items.len() - 1)]
    }
}

/// The last `count` Unicode scalar values of `text`.
fn tail(text: &str, count: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    chars[chars.len().saturating_sub(count)..].iter().collect()
}

/// What `mixed_ops` saw: the owner model built from claims accepted online (key → machine), and
/// every failure whose code is not an expected refusal, as (step, machine, arguments, code).
type Outcome = (
    BTreeMap<String, String>,
    Vec<(usize, String, Vec<String>, String)>,
);

/// `_v8_mixed_ops`: `steps` random claims, releases, edits and syncs on `machines` over `keys`,
/// drawn from `seed`; `after_remote_op` runs after each operation that reaches the remote (all but
/// the edits).
fn mixed_ops(
    machines: &[&Machine],
    keys: &[String],
    steps: usize,
    seed: u64,
    mut after_remote_op: Option<&mut dyn FnMut()>,
) -> Outcome {
    let mut rng = Rng(seed);
    let (mut owner, mut unexpected) = (BTreeMap::new(), Vec::new());
    for step in 0..steps {
        let machine = *rng.pick(machines);
        let key = rng.pick(keys).clone();
        let roll = rng.next();
        let (op, args) = if roll < 0.3 {
            ("sync", vec!["sync".to_owned()])
        } else if roll < 0.55 {
            ("claim", vec!["claim".to_owned(), key.clone()])
        } else if roll < 0.75 {
            ("release", vec!["release".to_owned(), key.clone()])
        } else {
            let notes = format!("step {step}");
            let words = ["update", &key, "--notes", &notes];
            ("update", words.map(str::to_owned).to_vec())
        };
        let run = machine.run(args.iter().map(String::as_str).chain(["--json"]));
        if run.returncode() == 0 {
            match op {
                "claim" => {
                    owner.insert(key, machine.name().to_owned());
                }
                "release" => {
                    owner.remove(&key);
                }
                _ => {}
            }
        } else {
            let code = parse_json(&run.stderr)
                .ok()
                .and_then(|error| error["error"]["code"].as_str().map(str::to_owned))
                .unwrap_or_else(|| tail(&run.stderr, 300));
            if !["ALREADY_CLAIMED", "NOT_OWNER", "ISSUE_NOT_FOUND"].contains(&code.as_str()) {
                unexpected.push((step, machine.name().to_owned(), args, code));
            }
        }
        if op != "update"
            && let Some(after) = after_remote_op.as_mut()
        {
            after();
        }
    }
    (owner, unexpected)
}

/// `word` quoted for `/bin/sh`.
fn quoted(word: &OsStr) -> String {
    let text = word.to_str().expect("a UTF-8 word");
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// `script(1)` running the shell command `line` with exactly `env`, on a new pseudo-terminal: its
/// `/bin/sh` is the session leader there, with the terminal as its controlling terminal and as its
/// standard input, output and error, and what the terminal shows is copied to script's standard
/// output. An `exec` in `line` makes the command it runs that session leader. BSD's script (macOS)
/// takes the command as arguments; util-linux's runs one line with `$SHELL`, which is `/bin/sh`
/// for it, and exits with the command's status (`-e`), as BSD's always does.
fn script(line: &str, env: &BTreeMap<OsString, OsString>) -> Command {
    let mut command = Command::new("script");
    command.env_clear().envs(env);
    if cfg!(target_os = "linux") {
        command
            .args(["-qec", line, "/dev/null"])
            .env("SHELL", "/bin/sh");
    } else {
        command.args(["-q", "/dev/null", "/bin/sh", "-c", line]);
    }
    command
}

/// What a pipe held, handed over by the thread that read it to its end.
type Drained = Receiver<Vec<u8>>;

/// A thread that reads `pipe` to its end, and the channel it hands what it read to.
fn drain(mut pipe: impl Read + Send + 'static) -> Drained {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = pipe.read_to_end(&mut bytes);
        let _ = sender.send(bytes);
    });
    receiver
}

/// How `child` ended, waiting at most `limit`: `None` if it is still running then.
fn wait_for(child: &mut Child, limit: Duration) -> Option<ExitStatus> {
    let deadline = Instant::now() + limit;
    loop {
        if let Some(status) = child.try_wait().expect("the child's status") {
            return Some(status);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
