//! The unpublished count on two machines (roll-lqjj.47): what each says is what its journal and
//! `meta.json` say, counted the slow way, after what `count` cannot do alone: another machine's
//! segments and attempt files merged in, a claim taken over and released, an acknowledgement lost
//! to a crash after the remote took the push, a sync that fails with writes on both machines, a
//! journal rotated, and the memo of each machine damaged.

use beadroll_store::memo_path;
use beadroll_test_support::{Machine, Sandbox};

use crate::count::{away_from, counts, damage};
use crate::support::{sandbox, workspace};

/// Two machines of one tracker, and an issue they share.
struct Pair<'a> {
    sandbox: &'a Sandbox,
    a: &'a Machine,
    b: &'a Machine,
    key: &'a str,
}

impl Pair<'_> {
    /// What each machine's store says of the count, each checked against the slow count.
    fn count(&self, when: &str) -> [Option<String>; 2] {
        [self.a, self.b]
            .map(|machine| counts(machine, self.sandbox, &format!("{} {when}", machine.name())))
    }

    /// Nothing either machine holds is unpublished.
    fn nothing_waits(&self, when: &str) {
        let nothing = [Some("0".to_owned()), Some("0".to_owned())];
        assert_eq!(self.count(when), nothing, "{when}");
    }

    /// Each machine syncs, in turn, and the count is nothing on both.
    fn sync_all(&self) {
        for machine in [self.a, self.b, self.a] {
            machine.bead(["sync"]);
        }
        self.nothing_waits("synced");
    }

    /// b's claim is merged by a, which then writes, takes the claim over and releases it, all
    /// but the first offline, and b merges what a made.
    fn foreign_merges_takeovers_and_releases(&self) {
        let (a, b, key) = (self.a, self.b, self.key);
        // b claims online, so that its attempt file is published; a has not merged it yet
        let claimed = b.run(["claim", key]);
        assert_eq!(claimed.returncode(), 0, "{}", claimed.stderr);
        self.nothing_waits("b claimed");
        // a's sync merges b's segment and claims directory: a foreign merge
        a.bead(["sync"]);
        assert_eq!(counts(a, self.sandbox, "a merged b"), Some("0".into()));
        a.bead(["comments", "add", key, "one", "--offline"]);
        assert_eq!(counts(a, self.sandbox, "a wrote"), Some("1".into()));

        let taken = a.run(["takeover", key, "-r", "mine"]);
        assert_eq!(taken.returncode(), 0, "{}", taken.stderr);
        counts(a, self.sandbox, "a took over");
        let released = a.run(["release", key, "--offline"]);
        assert_eq!(released.returncode(), 0, "{}", released.stderr);
        let waiting = counts(a, self.sandbox, "a released offline").expect("a count");
        assert!(waiting.parse::<i64>().unwrap() > 0, "the release waits");
        a.bead(["sync"]);
        assert_eq!(counts(a, self.sandbox, "a synced"), Some("0".into()));
        // b merges what a made, another machine's attempt files among it
        b.bead(["sync"]);
        self.nothing_waits("b merged a");
    }

    /// A claim whose acknowledgement is lost to a crash after the remote took the push, and a
    /// takeover that dies before its push and leaves its attempt pending.
    fn lost_acknowledgements(&self) {
        let (a, b, key) = (self.a, self.b, self.key);
        let lost = b
            .cmd(["claim", key])
            .env("BEAD_TEST_CRASH_AFTER_PUSH", "1")
            .run();
        assert_eq!(lost.returncode(), 99, "{}", lost.stderr);
        self.count("b lost an acknowledgement");
        b.bead(["sync"]);
        assert_eq!(counts(b, self.sandbox, "b synced"), Some("0".into()));

        let died = a
            .cmd(["takeover", key, "-r", "again"])
            .env("BEAD_TEST_CRASH_BEFORE_PUSH", "1")
            .run();
        assert_eq!(died.returncode(), 98, "{}", died.stderr);
        self.count("a died before its push");
        self.sync_all();
    }

    /// Writes waiting on both machines while the remote is away, a sync that fails, and the
    /// remote back.
    fn a_failed_sync(&self) {
        let origin = self.sandbox.origin();
        let away = away_from(origin);
        std::fs::rename(origin, &away).expect("the remote goes");
        self.a
            .bead(["comments", "add", self.key, "three", "--offline"]);
        self.b
            .bead(["comments", "add", self.key, "four", "--offline"]);
        let waiting = self.count("the remote is away");
        assert!(waiting.iter().all(|count| count.as_deref() != Some("0")));
        assert_ne!(self.a.run(["sync"]).returncode(), 0, "the remote is gone");
        assert_eq!(self.count("after a failed sync"), waiting);
        std::fs::rename(&away, origin).expect("the remote is back");
        self.sync_all();
    }

    /// A journal rotated by writes that publish, and what b makes of it.
    fn a_rotated_journal(&self) {
        for text in ["five", "six", "seven"] {
            let written = (self.a)
                .cmd(["comments", "add", self.key, text])
                .env("BEAD_ROTATE_BYTES", "1")
                .run();
            assert_eq!(written.returncode(), 0, "{}", written.stderr);
            let count = counts(self.a, self.sandbox, "a rotated");
            assert_eq!(count.as_deref(), Some("0"));
        }
        self.count("b before it merges the rotation");
        self.b.bead(["sync"]);
        self.nothing_waits("b merged the rotation");
    }

    /// The memo of two stores that merged each other's files, damaged or taken away: the count
    /// is the same.
    fn damaged_memos(&self) {
        for how in [
            "an attempt file's fields",
            "garbage",
            "older than every file",
            "trusted for every file",
            "missing",
        ] {
            for machine in [self.a, self.b] {
                let ws = workspace(machine, self.sandbox.origin(), &[]);
                if memo_path(&ws).exists() {
                    damage(&ws, how);
                }
            }
            self.nothing_waits(how);
        }
    }
}

#[test]
fn the_count_is_the_slow_count_after_foreign_merges_takeovers_and_lost_acknowledgements() {
    let sandbox = sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let key = a.bead(["create", "x"]).expect("a create")["id"]
        .as_str()
        .expect("a key")
        .to_owned();
    let b = Machine::new(&sandbox, "b");
    b.bead(["sync"]);
    let pair = Pair {
        sandbox: &sandbox,
        a: &a,
        b: &b,
        key: &key,
    };
    pair.nothing_waits("joined");

    pair.foreign_merges_takeovers_and_releases();
    pair.lost_acknowledgements();
    pair.a_failed_sync();
    pair.a_rotated_journal();
    pair.damaged_memos();
}
