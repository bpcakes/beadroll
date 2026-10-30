//! A six-machine reference model running 2,400 seeded commands sequentially
//! (`docs/reviews/2026-10-06-1-audit.md`). Record accepted claims from command results. Every 300
//! commands, settle the fleet and compare exports, details, readiness, owners and generations,
//! checking that no accepted claim disappears from journals or claim history.
//!
//! Every 160 commands, select a machine for a soft partition lasting 40–99 commands: use offline
//! claims and substitute reads for sync, release and takeover. Closes are offline and limited to
//! unowned issues. Seed 20261006 reproduces the command sequence; draws depend only on the random
//! stream and number of issues. All machines use this build's binary because checks read stores
//! in-process.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use beadroll_core::id::EventId;
use beadroll_sync::NetworkMode;
use beadroll_test_support::{Json, Machine, Run};

use super::{
    Rng, details, each, error_code, id, machines, missing, realized, replayed, reply, run_json,
    sorted, status_disputed, words,
};

/// What a step of the model does.
#[derive(Clone, Copy, Debug)]
enum Action {
    Show,
    Update,
    List,
    Ready,
    Close,
    Create,
    Dep,
    Comment,
    Claim,
    Release,
    Takeover,
    Sync,
}

/// Actions and their selection weights.
const ACTIONS: [(Action, usize); 12] = [
    (Action::Show, 32),
    (Action::Update, 12),
    (Action::List, 9),
    (Action::Ready, 6),
    (Action::Close, 5),
    (Action::Create, 5),
    (Action::Dep, 5),
    (Action::Comment, 4),
    (Action::Claim, 6),
    (Action::Release, 4),
    (Action::Takeover, 3),
    (Action::Sync, 9),
];

/// The codes a refused command may report: domain refusals, never a failure of bead.
const ALLOWED: [&str; 7] = [
    "ISSUE_NOT_FOUND",
    "VALIDATION",
    "STATUS_CONFLICT",
    "ALREADY_CLAIMED",
    "NOT_OWNER",
    "CLAIMED_BY_OTHER",
    "CYCLE",
];

/// The claims the remote accepted, as the commands that made them reported them.
#[derive(Default)]
struct Model {
    /// The expected current claim for each owned issue.
    holder: BTreeMap<String, String>,
    /// The machine that made it (`model_owner`).
    owner: BTreeMap<String, usize>,
    /// Each issue's last claim generation (`generations`).
    generations: BTreeMap<String, i64>,
    /// Every claim the remote accepted (`accepted`).
    accepted: BTreeSet<String>,
}

/// One step's command, as the transcript keeps it.
struct Row<'a> {
    step: usize,
    machine: usize,
    partitioned: bool,
    args: &'a [String],
    exit: i32,
}

impl std::fmt::Debug for Row<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let cut = if self.partitioned {
            " (partitioned)"
        } else {
            ""
        };
        let (step, machine, args, exit) = (self.step, self.machine, self.args, self.exit);
        write!(
            f,
            "step {step}, machine {machine}{cut}: {args:?} exited {exit}"
        )
    }
}

/// What one machine showed at a full check.
struct Seen {
    export: String,
    view: String,
    ready: BTreeSet<String>,
    events: i64,
}

/// A full-check summary and when it passed.
struct Check {
    step: usize,
    issues: usize,
    accepted_claims: usize,
    events: i64,
    seconds: f64,
}

impl std::fmt::Display for Check {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (step, issues, claims) = (self.step, self.issues, self.accepted_claims);
        let (events, seconds) = (self.events, self.seconds);
        write!(
            f,
            "step {step}: {issues} issues, {claims} accepted claims, {events} events \
             ({seconds:.1} s)"
        )
    }
}

/// The model's run: the fleet, the issues, the model and what happened so far.
struct Soak {
    machines: Vec<Machine>,
    keys: Vec<String>,
    model: Model,
    /// Each partitioned machine, and the last step it stays partitioned for.
    partitions: BTreeMap<usize, usize>,
    counts: BTreeMap<String, usize>,
    errors: BTreeMap<String, usize>,
    checks: Vec<Check>,
    started: Instant,
}

/// `audit_model_run`: `operations` commands drawn from `seed` on six machines, then the run's
/// summary.
fn audit_model_run(operations: usize, seed: u64) -> Soak {
    let started = Instant::now();
    let sandbox = crate::sandbox();
    let machines = machines(&sandbox, "soak");
    let root = &machines[0];
    root.bead(["init", "--prefix", "soak"]);
    let create = |title: String| id(&root.bead(["create", &title]).expect("an issue"));
    let keys = (0..24).map(|n| create(format!("seed issue {n}"))).collect();
    for machine in &machines {
        machine.bead(["sync"]);
    }
    let mut soak = Soak {
        machines,
        keys,
        model: Model::default(),
        partitions: BTreeMap::new(),
        counts: BTreeMap::new(),
        errors: BTreeMap::new(),
        checks: Vec::new(),
        started,
    };
    let mut rng = Rng::new(seed);
    for step in 1..=operations {
        soak.step(&mut rng, step);
        if step % 300 == 0 || step == operations {
            soak.full_check(step);
        }
    }
    eprintln!(
        "passed: {operations} operations on 6 machines, seed {seed}, {:.1} s; counts {:?}; \
         errors {:?}; {} accepted online claims",
        started.elapsed().as_secs_f64(),
        soak.counts,
        soak.errors,
        soak.model.accepted.len()
    );
    soak
}

impl Soak {
    /// Step `step`: perhaps a new partition, then one command from a machine drawn at random.
    fn step(&mut self, rng: &mut Rng, step: usize) {
        if step % 160 == 1 {
            let chosen = rng.below(6);
            self.partitions.insert(chosen, step + rng.range(40, 100));
        }
        let mi = rng.below(6);
        let offline = self.partitions.get(&mi).is_some_and(|&end| end >= step);
        let args = self.command(
            rng,
            step,
            mi,
            if offline {
                NetworkMode::Offline
            } else {
                NetworkMode::Online
            },
        );
        *self.counts.entry(args[0].clone()).or_default() += 1;
        let run = run_json(&self.machines[mi], &args);
        let row = Row {
            step,
            machine: mi,
            partitioned: offline,
            args: &args,
            exit: run.returncode(),
        };
        if row.exit == 0 {
            self.record(&row, &run);
        } else {
            let code = error_code(&run)
                .unwrap_or_else(|| panic!("{row:?}: unstructured error: {}", run.stderr));
            assert!(ALLOWED.contains(&code.as_str()), "{row:?}: {}", run.stderr);
            *self.errors.entry(code).or_default() += 1;
        }
    }

    /// The action applied to a randomly selected key at this step.
    fn command(&self, rng: &mut Rng, step: usize, mi: usize, offline: NetworkMode) -> Vec<String> {
        let offline = matches!(offline, NetworkMode::Offline);
        let action = rng.weighted(&ACTIONS);
        let key = rng.choice(&self.keys).as_str();
        match action {
            Action::Show => words(["show", key]),
            Action::List => words(["list", "--all", "--limit", "0"]),
            Action::Ready => words(["ready"]),
            Action::Update => {
                let notes = format!("step {step} machine {mi}");
                let priority = rng.below(5).to_string();
                let edit = *rng.choice(&["--add-label", "--remove-label"]);
                let label = *rng.choice(&["alpha", "beta", "gamma"]);
                words([
                    "update",
                    key,
                    "--notes",
                    &notes,
                    "--priority",
                    &priority,
                    edit,
                    label,
                ])
            }
            Action::Create => words(["create", &format!("generated {step}")]),
            // Random closes keep to issues without a confirmed model owner; late closes and
            // takeovers and forced transitions have regressions of their own.
            Action::Close if self.model.holder.contains_key(key) => words(["show", key]),
            Action::Close => words(["close", key, "--offline"]),
            Action::Dep => {
                let target = rng.choice(&self.keys).as_str();
                words(["dep", rng.choice(&["add", "remove"]), key, target])
            }
            Action::Comment => words(["comments", "add", key, &format!("synthetic note {step}")]),
            Action::Sync if offline => words(["info"]),
            Action::Sync => words(["sync"]),
            Action::Claim if offline => words(["claim", key, "--offline"]),
            Action::Claim => words(["claim", key]),
            Action::Release | Action::Takeover if offline => words(["show", key]),
            Action::Release => words(["release", key]),
            Action::Takeover => words(["takeover", key, "--reason", "synthetic model test"]),
        }
    }

    /// What a successful command changes in the model: a new issue joins the keys; an online
    /// claim or takeover is the issue's claim now, one generation on unless it is the claim the
    /// issue already had; a release by the machine whose claim the model has ends that claim.
    fn record(&mut self, row: &Row<'_>, run: &Run) {
        let result = reply(run, row);
        let key = row.args.get(1).cloned().unwrap_or_default();
        match row.args[0].as_str() {
            "create" => self.keys.push(id(&result)),
            "claim" | "takeover" if !row.partitioned => {
                let held = result[0].get("claim");
                let held = held.unwrap_or_else(|| panic!("{row:?}: no claim in {result:?}"));
                let claim = id(held);
                let generation = held["generation"].as_i64();
                if self.model.holder.get(&key) != Some(&claim) {
                    let last = self.model.generations.get(&key).copied().unwrap_or(0);
                    assert_eq!(generation, Some(last + 1), "{row:?}: {held:?}");
                }
                let generation = generation.unwrap_or_else(|| panic!("{row:?}: {held:?}"));
                self.model.generations.insert(key.clone(), generation);
                self.model.holder.insert(key.clone(), claim.clone());
                self.model.owner.insert(key, row.machine);
                self.model.accepted.insert(claim);
            }
            "release" if self.model.owner.get(&key) == Some(&row.machine) => {
                self.model.holder.remove(&key);
                self.model.owner.remove(&key);
            }
            _ => {}
        }
    }

    /// `full_check`: every machine syncs twice, with no forked journal and no event waiting for
    /// its causes; then each machine's export, details and ready set agree with every other's,
    /// its claims with the model, its readiness with each issue's status, blockers and disputes,
    /// and its journal and claims with every claim the model accepted.
    fn full_check(&mut self, step: usize) {
        for machine in self.machines.iter().chain(&self.machines) {
            let result = machine.bead(["sync"]).expect("a sync report");
            assert!(!result["forked_journals"].truthy(), "{step}: {result:?}");
            assert!(!result["waiting_events"].truthy(), "{step}: {result:?}");
        }
        let seen = each(&self.machines, |_, machine| self.seen_by(step, machine));
        assert!(
            seen.iter().all(|s| s.export == seen[0].export),
            "{step}: exports diverged"
        );
        assert!(
            seen.iter().all(|s| s.view == seen[0].view),
            "{step}: details diverged"
        );
        assert!(
            seen.iter().all(|s| s.ready == seen[0].ready),
            "{step}: readiness diverged"
        );
        let check = Check {
            step,
            issues: self.keys.len(),
            accepted_claims: self.model.accepted.len(),
            events: seen.last().map_or(0, |s| s.events),
            seconds: self.started.elapsed().as_secs_f64(),
        };
        eprintln!("checkpoint at {check}");
        self.checks.push(check);
    }

    /// One machine's part of a full check: what it shows, each issue checked against the model,
    /// and its store against the accepted claims.
    fn seen_by(&self, step: usize, machine: &Machine) -> Seen {
        let export = machine.export();
        let info = machine.bead(["info"]).expect("an info report");
        let quiet = !info["waiting_events"].truthy() && !info["integrity"].truthy();
        assert!(quiet, "{step}: {} {info:?}", machine.name());
        let details = details(machine, &self.keys);
        let ready = machine.ready();
        for issue in details.as_array().expect("bead show prints a list") {
            self.check_issue(step, machine, issue, &ready);
        }
        self.check_store(step, machine);
        Seen {
            export,
            view: sorted(&details),
            ready,
            events: info["events"].as_i64().expect("a count of events"),
        }
    }

    /// An issue as `machine` shows it: claimed as the model has it, and ready exactly when it is
    /// open, unblocked and its status undisputed.
    fn check_issue(&self, step: usize, machine: &Machine, issue: &Json, ready: &BTreeSet<String>) {
        let key = id(issue);
        let current = issue.get("claim").filter(|claim| claim.truthy());
        let context = || format!("{step}: {} {issue:?}", machine.name());
        assert_eq!(
            current.map(id).as_ref(),
            self.model.holder.get(&key),
            "{}",
            context()
        );
        if let Some(current) = current {
            let generation = self.model.generations.get(&key).copied();
            assert_eq!(current["generation"].as_i64(), generation, "{}", context());
        }
        let expect_ready = issue["status"].as_str() == Some("open")
            && !issue["blocked"].truthy()
            && !status_disputed(issue);
        assert_eq!(ready.contains(&key), expect_ready, "{}", context());
    }

    /// `machine`'s store, replayed in-process: its journal holds every accepted claim's event, and
    /// each is the current claim of its issue or in its claim history.
    fn check_store(&self, step: usize, machine: &Machine) {
        let state = replayed(machine);
        let lost: Vec<&String> = (self.model.accepted.iter())
            .filter(|claim| !EventId::parse(claim).is_some_and(|e| state.holds(e)))
            .collect();
        assert!(lost.is_empty(), "{step}: lost accepted event {lost:?}");
        let gone = missing(&self.model.accepted, &realized(&state));
        assert!(
            gone.is_empty(),
            "{step}: accepted claim missing from current/history {gone:?}"
        );
    }
}

/// 2,400 seeded commands from six machines against the reference model of accepted claims, with
/// eight full checks.
#[test]
#[ignore = "stress: randomised models"]
fn six_machine_2400_operation_reference_model() {
    let soak = audit_model_run(2400, 20_261_006);
    assert_eq!(soak.checks.len(), 8);
    let operations: usize = soak.counts.values().sum();
    assert_eq!(operations, 2400);
}
