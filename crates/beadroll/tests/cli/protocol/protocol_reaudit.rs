//! `ProtocolReaudit`, black-box: an ownership attempt whose push the remote took while bead heard
//! nothing back (the acknowledgement lost, the client killed at its timeout, or git exiting 1
//! after the push) settles one way or the other, whole, and no journal forks (the second
//! independent audit, 2026-10-06). The class's in-process tests are in `beadroll-sync`
//! (`tests/sync/protocol/protocol_reaudit.rs`).

use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use beadroll_store::glob_ending;
use beadroll_test_support::{Json, Machine, Sandbox, code, git, parse_json};

/// The hook that stops a command after the remote took its push, before bead hears back.
const CRASH_AFTER_PUSH: &str = "BEAD_TEST_CRASH_AFTER_PUSH";

/// Machine a's tracker with one issue, "ownership target", synced to machine b: the class's
/// `setUp`, less the in-process store its other tests open.
struct Reaudit {
    sandbox: Sandbox,
    a: Machine,
    b: Machine,
    key: String,
}

fn setup() -> Reaudit {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let created = a.bead(["create", "ownership target"]).unwrap();
    let key = created["id"]
        .as_str()
        .expect("the new issue's id")
        .to_owned();
    a.bead(["sync"]);
    let b = Machine::new(&sandbox, "b");
    b.bead(["sync"]);
    Reaudit { sandbox, a, b, key }
}

impl Reaudit {
    /// The ids of the events of machine a's unsettled attempt: every line of every attempt file in
    /// its `pending/`, ownership events and their edits alike.
    fn pending_ids(&self) -> Vec<String> {
        let mut ids = Vec::new();
        for path in glob_ending(&self.a.store().join("pending"), ".json") {
            let text = std::fs::read_to_string(&path).unwrap();
            for line in text.lines().filter(|line| !line.trim().is_empty()) {
                let event = parse_json(line).unwrap();
                ids.push(event["id"].as_str().expect("an event id").to_owned());
            }
        }
        ids
    }

    /// Both machines sync twice; then they agree, and the issue is what they agree on.
    fn settle(&self) -> Json {
        for machine in [&self.a, &self.b, &self.a, &self.b] {
            machine.bead(["sync"]);
        }
        assert_eq!(self.a.export(), self.b.export());
        assert_eq!(self.a.show(&self.key), self.b.show(&self.key));
        self.a.show(&self.key)
    }

    /// The commit the remote's coordination ref holds.
    fn remote_tip(&self) -> String {
        let args = [
            OsStr::new("--git-dir"),
            self.sandbox.origin().as_os_str(),
            OsStr::new("rev-parse"),
            OsStr::new("refs/beads/journal"),
        ];
        git(args, None).trim().to_owned()
    }

    /// The `try` block of the delayed-push test: a claim, then a sync, each giving up on a push
    /// the hook holds, leave the attempt pending and the claim unseen; local work goes on.
    fn stalled_claim_stays_pending(&self, started: &Path, finished: &Path) {
        let result = self
            .a
            .cmd(["claim", &self.key, "--json"])
            .env("BEAD_NET_TIMEOUT", "0.3")
            .run();
        assert_eq!(result.returncode(), 1, "{}", result.stderr);
        let answer = code(&result);
        assert!(
            ["REMOTE_UNREACHABLE", "UNRESOLVED"].contains(&answer.as_str()),
            "{answer}"
        );
        assert!(started.exists(), "timed out before receive hook");
        assert!(!finished.exists(), "hook did not remain in flight");
        assert_eq!(
            self.pending_ids().len(),
            1,
            "the unanswered attempt must stay pending"
        );
        assert!(self.a.show(&self.key).get("claim").is_none());
        // the fence stalls behind the hook too
        let stalled = self
            .a
            .cmd(["sync", "--json"])
            .env("BEAD_NET_TIMEOUT", "0.3")
            .run();
        assert_eq!(stalled.returncode(), 1, "{}", stalled.stdout);
        assert_eq!(code(&stalled), "REMOTE_UNREACHABLE");
        assert_eq!(
            self.pending_ids().len(),
            1,
            "an unsettled attempt is never dropped on a guess"
        );
        let note = "work while the attempt is unsettled";
        self.a.bead(["comments", "add", &self.key, note]);
    }
}

/// `script` written to `path`, executable.
fn executable(path: &Path, script: &str) {
    std::fs::write(path, script).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// `path` quoted for the shell.
fn quoted(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
}

/// The git `which git` finds: the first executable file of that name on the test's PATH.
fn real_git() -> PathBuf {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let runnable = |git: &PathBuf| {
        std::fs::metadata(git).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    };
    std::env::split_paths(&path)
        .map(|dir| dir.join("git"))
        .find(runnable)
        .expect("git on PATH")
}

/// The texts of an issue's comments, as `show` gives them.
fn comments(issue: &Json) -> Vec<&str> {
    let comments = issue["comments"].as_array().expect("the issue's comments");
    comments
        .iter()
        .map(|comment| comment["text"].as_str().expect("a comment's text"))
        .collect()
}

/// Touches its file when dropped: the hook is released whatever the test came to.
struct Touch(PathBuf);

impl Drop for Touch {
    fn drop(&mut self) {
        let _ = std::fs::write(&self.0, b"");
    }
}

/// A claiming update that also sets the title and adds a label, whose acknowledgement is lost
/// after the remote took it, counts for nothing here until a sync finds it on the remote; then it
/// applies as one transaction on every machine (the claim, the title and the label), with no
/// integrity problem.
#[test]
fn multievent_update_lost_ack_applies_whole_transaction() {
    let s = setup();
    let crashed =
        s.a.cmd([
            "update",
            &s.key,
            "--claim",
            "--title",
            "accepted title",
            "--add-label",
            "accepted",
        ])
        .env(CRASH_AFTER_PUSH, "1")
        .run();
    assert_eq!(crashed.returncode(), 99, "{}", crashed.stderr);
    let before = s.a.show(&s.key);
    assert_eq!(before["title"].as_str(), Some("ownership target"));
    assert!(before.get("claim").is_none(), "{before:?}");
    let last = s.settle();
    assert_eq!(last["title"].as_str(), Some("accepted title"));
    assert_eq!(last["labels"], Json::from(vec![Json::from("accepted")]));
    assert_eq!(last["claim"]["thread"].as_str(), Some("a-thread-0001"));
    assert_eq!(
        s.a.bead(["info"]).unwrap()["integrity"],
        Json::Array(Vec::new())
    );
}

/// A push the client gave up on may still land later. The claim is pending meanwhile; the fences
/// bead pushes to settle it also stall behind the same hook, so when the hook releases, one of
/// several commits on the same head wins. Either way the attempt settles and no journal forks.
#[test]
fn delayed_push_after_timeout_and_absent_fetch_must_not_fork() {
    let s = setup();
    let root = s.sandbox.root();
    let started = root.join("hook-started");
    let release = root.join("hook-release");
    let finished = root.join("hook-finished");
    let hook = s.sandbox.origin().join("hooks").join("pre-receive");
    let script = format!(
        "#!/bin/sh\ntouch {}\nn=0\nwhile [ ! -f {} ] && [ \"$n\" -lt 400 ]; do sleep 0.05; \
         n=$((n+1)); done\ntouch {}\nexit 0\n",
        quoted(&started),
        quoted(&release),
        quoted(&finished)
    );
    executable(&hook, &script);
    let previous_tip = s.remote_tip();
    let released = Touch(release);
    s.stalled_claim_stays_pending(&started, &finished);
    drop(released); // the `finally`: release the hook, then wait for a push to land
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut current = String::new();
    while Instant::now() < deadline {
        current = s.remote_tip();
        if current != previous_tip {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_ne!(
        current, previous_tip,
        "fault injection did not produce a delayed successful push"
    );
    std::thread::sleep(Duration::from_millis(500)); // let the losing receive-packs finish failing
    let recovered = s.a.run(["sync", "--json"]);
    assert_eq!(
        recovered.returncode(),
        0,
        "late remote acceptance collided with new local tail: {}",
        recovered.stderr
    );
    assert_eq!(
        s.pending_ids(),
        Vec::<String>::new(),
        "the attempt is settled one way or the other"
    );
    let last = s.settle();
    assert_eq!(comments(&last), ["work while the attempt is unsettled"]);
    assert_eq!(
        s.a.bead(["info"]).unwrap()["integrity"],
        Json::Array(Vec::new())
    );
    if last.get("claim").is_some() {
        // the delayed claim won the race: it is confirmed everywhere
        assert_eq!(last["claim"]["thread"].as_str(), Some("a-thread-0001"));
    } else {
        // a fence won: the claim died in flight and nobody ever saw it
        for machine in [&s.a, &s.b] {
            let claims = machine.bead(["claims"]).unwrap();
            assert_eq!(claims["claims"], Json::Array(Vec::new()));
        }
    }
}

/// git's status line outranks its exit status: a push the remote accepted, after which git exits
/// 1 as if the transport's acknowledgement were lost, counts as accepted. The claim succeeds, it is
/// the claim the other machine sees, and later work and a sync keep it, with no integrity problem.
#[test]
fn successful_push_with_nonzero_exit_must_preserve_accepted_claim() {
    let s = setup();
    let wrapper = s.sandbox.root().join("git-wrapper");
    std::fs::create_dir(&wrapper).unwrap();
    let git = quoted(&real_git());
    // Run a real local push, then inject a lost transport acknowledgement as status 1.
    let script = format!(
        "#!/bin/sh\nif [ \"$1\" = push ]; then\n  {git} \"$@\" || exit $?\n  exit 1\nfi\nexec \
         {git} \"$@\"\n"
    );
    executable(&wrapper.join("git"), &script);
    let mut path = wrapper.into_os_string();
    path.push(":");
    path.push(std::env::var_os("PATH").expect("the test's PATH"));
    let result = s.a.cmd(["claim", &s.key, "--json"]).env("PATH", path).run();
    s.b.bead(["sync"]);
    let shown = s.b.show(&s.key);
    let accepted = shown.get("claim").expect("b sees the claim").clone();
    assert_eq!(result.returncode(), 0, "{}", result.stderr);
    s.a.bead(["comments", "add", &s.key, "work after retry"]);
    assert_eq!(s.a.bead(["sync"]).unwrap()["integrity"].as_i64(), Some(0));
    assert_eq!(s.a.show(&s.key)["claim"], accepted);
}
