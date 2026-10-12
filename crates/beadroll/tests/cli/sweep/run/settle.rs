//! Settling (Python's `settle`): every machine synced twice, then compared with the others, and
//! nothing of the sandbox left running.

use std::collections::BTreeMap;
use std::process::Command;

use beadroll_test_support::Json;

use super::{Run, lock, shown};
use crate::sweep::call::tail;

/// What one machine shows once settled: its export, `show` of every issue, and its claims.
type Seen = (String, Option<Json>, Vec<Json>);

impl Run {
    /// Two rounds of syncs everywhere; then every machine must show the same tracker (exports,
    /// `show`, `claims`), with no integrity entries, waiting events or pending attempts, no sync
    /// may have reported a forked journal, and no process of the sandbox may be left running.
    pub(super) fn settle(&self) {
        for _ in 0..2 {
            for (machine, thread) in self.machines() {
                let step = self.call(machine, &["sync"], thread, None, "settle");
                if !step.rc.is(0) {
                    let code = step.code.as_deref().unwrap_or("None");
                    let said = tail(&step.err, 200);
                    let name = machine.name();
                    self.violation(format!("settling sync on {name} failed: {code} {said}"));
                }
            }
        }
        self.observe("settle");
        let keys = ['Q', 'X', 'Y', 'Z', 'P'].map(|letter| self.keys.of(letter));
        let show = [&["show"][..], &keys[..]].concat();
        let mut seen = BTreeMap::new();
        for (machine, thread) in self.machines() {
            let export = Run::export(machine);
            let shows = self.call(machine, &show, thread, None, "final show").out;
            let listed = self
                .call(machine, &["claims"], thread, None, "final claims")
                .out;
            let claims = claim_rows(listed.as_ref());
            let info = self
                .call(machine, &["info"], thread, None, "final info")
                .out;
            let info = info.unwrap_or_else(|| Json::Object(Default::default()));
            let (integrity, waiting, pending) = (
                &info["integrity"],
                &info["waiting_events"],
                &info["pending_ownership"],
            );
            if integrity.truthy() || waiting.truthy() || pending.truthy() {
                let (integrity, waiting, pending) =
                    (shown(integrity), shown(waiting), shown(pending));
                let name = machine.name();
                self.violation(format!(
                    "{name} after settling: integrity {integrity}, waiting {waiting}, pending {pending}"
                ));
            }
            seen.insert(machine.name(), (export, shows, claims));
        }
        self.compare(&seen);
        let forks = lock(&self.forks).clone();
        if !forks.is_empty() {
            let forks: Vec<String> = forks
                .iter()
                .map(|(step, forked)| format!("({step}, {})", shown(forked)))
                .collect();
            self.violation(format!("forked_journals reported: {}", forks.join(", ")));
        }
        self.stray_processes();
    }

    /// m2 and m3 must agree with m1: exports, `show` of every issue, and claims.
    fn compare(&self, seen: &BTreeMap<&str, Seen>) {
        let first = &seen["m1"];
        for name in ["m2", "m3"] {
            let other = &seen[name];
            if other.0 != first.0 {
                self.violation(format!("export differs between m1 and {name}"));
            }
            if other.1 != first.1 {
                self.violation(format!("show differs between m1 and {name}"));
            }
            if other.2 != first.2 {
                self.violation(format!("claims differ between m1 and {name}"));
            }
        }
    }

    /// No process of the sandbox may outlive the schedule.
    fn stray_processes(&self) {
        let listing = Command::new("ps")
            .args(["-eo", "pid=,stat=,args="])
            .output()
            .expect("ps runs");
        let listing = String::from_utf8_lossy(&listing.stdout);
        let root = self.sandbox.root().to_string_lossy();
        let left: Vec<&str> = listing
            .lines()
            .filter(|row| row.contains(root.as_ref()) && !row.contains("ps -eo"))
            .collect();
        if !left.is_empty() {
            self.violation(format!("processes left behind: {left:?}"));
        }
    }
}

/// The rows of `bead claims`, without the two fields that differ from machine to machine:
/// `idle_minutes`, which moves with the clock, and `machine`, a local alias.
fn claim_rows(listed: Option<&Json>) -> Vec<Json> {
    let rows = listed.and_then(|listed| listed["claims"].as_array());
    let rows = rows.unwrap_or_default().iter().filter_map(Json::as_object);
    rows.map(|row| {
        let kept = row
            .iter()
            .filter(|(key, _)| !matches!(key.as_str(), "idle_minutes" | "machine"))
            .map(|(key, value)| (key.clone(), value.clone()));
        Json::Object(Box::new(kept.collect()))
    })
    .collect()
}
