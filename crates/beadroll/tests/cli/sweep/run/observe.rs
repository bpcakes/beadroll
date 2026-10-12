//! Compares every machine's observations with the remote as seen by a fresh reader.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use beadroll_test_support::{Json, parse_json};

use super::{OBS, Remote, Run, lock, shown};

impl Run {
    /// Local views (no network) of X on every machine, then the remote as a fresh reader sees it:
    /// a claim a machine counts must be one the remote accepted, its generation never lower than
    /// one shown before, and no claim lost.
    pub(super) fn observe(&self, after: &str) -> Remote {
        let x = self.keys.of('X');
        let label = format!("view after {after}");
        let views = self.machines().map(|(machine, thread)| {
            let step = self.call(machine, &["show", x], thread, None, &label);
            (machine.name(), step.first())
        });
        let remote = self.remote();
        let accepted = self.accepted_ids();
        for (name, view) in views {
            let claim = &view["claim"];
            if claim.truthy() {
                let id = claim["id"].as_str().expect("a claim's id");
                if !accepted.contains(id) {
                    let text = format!(
                        "after {after}: {name} counts claim {id} the remote never accepted"
                    );
                    self.violation(text);
                }
                let generation = claim["generation"].as_i64().expect("a claim's generation");
                let mut generations = lock(&self.generations);
                let history = generations.entry(name.to_owned()).or_default();
                if let Some(last) = history.last().copied()
                    && generation < last
                {
                    let text = format!(
                        "after {after}: {name} generation went back {last} -> {generation}"
                    );
                    self.violation(text);
                }
                history.push(generation);
            }
            if view["lost_claims"].truthy() {
                let lost = shown(&view["lost_claims"]);
                self.violation(format!("after {after}: {name} shows lost_claims {lost}"));
            }
        }
        remote
    }

    /// X, Y and Z as the observer sees them once it has synced: empty, and a violation, when it
    /// could not read the remote.
    fn remote(&self) -> Remote {
        let step = self.call(&self.obs, &["sync"], OBS, None, "observer sync");
        if !step.rc.is(0) {
            let text = format!("observer could not read the remote: {}", step.err);
            self.violation(text);
            return Remote::new();
        }
        let keys = ['X', 'Y', 'Z'].map(|letter| self.keys.of(letter));
        let argv = [&["show"][..], &keys[..]].concat();
        let shown = self.call(&self.obs, &argv, OBS, None, "observer show");
        let items = shown
            .out
            .as_ref()
            .and_then(Json::as_array)
            .unwrap_or_default();
        ['X', 'Y', 'Z']
            .into_iter()
            .zip(items.iter().cloned())
            .collect()
    }

    /// The ids of every event in an attempt the remote accepted: the observer's
    /// `events/*/claims/*.json`.
    fn accepted_ids(&self) -> BTreeSet<String> {
        let mut ids = BTreeSet::new();
        let events = self.obs.store().join("events");
        for journal in entries(&events) {
            for file in entries(&journal.join("claims")) {
                if !file.to_string_lossy().ends_with(".json") {
                    continue;
                }
                let text = std::fs::read_to_string(&file).expect("an accepted attempt");
                for line in text.lines().filter(|line| !line.trim().is_empty()) {
                    let event = parse_json(line).expect("an accepted attempt's event");
                    ids.insert(event["id"].as_str().expect("an event id").to_owned());
                }
            }
        }
        ids
    }
}

/// The entries of `dir`, or none when it cannot be read.
fn entries(dir: &Path) -> Vec<PathBuf> {
    let Ok(listing) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    listing
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect()
}
