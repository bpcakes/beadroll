//! A six-machine concurrency model (`docs/reviews/2026-10-06-2-reaudit.md`) with 24 editable
//! issues and 12 issues reserved for ownership operations. Ten waves run fifty commands per
//! machine concurrently. Machines `wave % 6` and `(wave + 2) % 6` substitute reads for ownership
//! and sync while their other writes publish normally.
//!
//! After each wave, sync twice and compare exports, details, pending state and integrity across
//! the fleet. Successful command receipts determine expected owners and monotonic generations. An
//! independent detail-based oracle checks blockers and readiness, and every accepted claim must
//! remain visible. Worker seeds are `20261007 + wave * 101 + machine * 907`; each worker has its
//! own deterministic random stream and command interleaving.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use beadroll_test_support::{Json, Machine, Run};

use self::oracle::{check_claims, expected_ready, exported_keys, settled};
use super::{
    Rng, details, each, error_code, id, joined, machines, missing, realized, reply, run_json,
    sorted, viewed, words,
};

mod oracle;

/// What a worker's step does.
#[derive(Clone, Copy, Debug)]
enum Action {
    Show,
    List,
    Ready,
    Edit,
    Comment,
    Create,
    Dep,
    Status,
    Claim,
    Release,
    Takeover,
    Sync,
}

/// The worker's actions and selection weights.
const ACTIONS: [(Action, usize); 12] = [
    (Action::Show, 28),
    (Action::List, 8),
    (Action::Ready, 6),
    (Action::Edit, 14),
    (Action::Comment, 6),
    (Action::Create, 5),
    (Action::Dep, 5),
    (Action::Status, 5),
    (Action::Claim, 8),
    (Action::Release, 5),
    (Action::Takeover, 4),
    (Action::Sync, 6),
];

/// The codes a refused command may report: domain refusals, and the remote moving or failing
/// under six machines at once; never a failure of bead.
const ALLOWED: [&str; 9] = [
    "ALREADY_CLAIMED",
    "NOT_OWNER",
    "CLAIMED_BY_OTHER",
    "VALIDATION",
    "STATUS_CONFLICT",
    "CYCLE",
    "ISSUE_NOT_FOUND",
    "SYNC_CONFLICT",
    "REMOTE_UNREACHABLE",
];

/// The issues the workers draw from.
struct Keys {
    /// Editable issues, including every issue a worker creates.
    editable: Vec<String>,
    /// The issues only claimed, taken over and released (`owned`).
    owned: Vec<String>,
    /// Every issue (`allkeys`).
    all: Vec<String>,
}

/// A command a worker ran, as the transcript keeps it.
struct Row {
    wave: usize,
    machine: usize,
    step: usize,
    partitioned: bool,
    args: Vec<String>,
    exit: i32,
    error: Option<String>,
}

impl std::fmt::Debug for Row {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let cut = if self.partitioned {
            " (partitioned)"
        } else {
            ""
        };
        let (wave, machine, step) = (self.wave, self.machine, self.step);
        let (args, exit, error) = (&self.args, self.exit, &self.error);
        write!(
            f,
            "wave {wave}, machine {machine}{cut}, step {step}: {args:?} exited {exit} {error:?}"
        )
    }
}

/// A successful claim, takeover or release, and the claim it took or ended.
struct Receipt {
    op: String,
    key: String,
    generation: i64,
    owner: String,
    id: String,
}

impl std::fmt::Debug for Receipt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (op, key, generation) = (&self.op, &self.key, self.generation);
        let (owner, id) = (&self.owner, &self.id);
        write!(
            f,
            "{op} {key}: claim {id} of {owner}, generation {generation}"
        )
    }
}

impl Receipt {
    /// `op` on `key`, which took or ended the claim `held`.
    fn of(op: &str, key: &str, held: &Json) -> Receipt {
        let generation = held["generation"].as_i64();
        Receipt {
            op: op.to_owned(),
            key: key.to_owned(),
            generation: generation.unwrap_or_else(|| panic!("no generation in {held:?}")),
            owner: held["owner"].as_str().unwrap_or_default().to_owned(),
            id: id(held),
        }
    }

    fn released(&self) -> bool {
        self.op == "release"
    }
}

/// One worker's results, created keys and receipts for a wave.
#[derive(Default)]
struct Done {
    rows: Vec<Row>,
    newkeys: Vec<String>,
    receipts: Vec<Receipt>,
}

/// One machine's fifty commands in a wave. `claims` maps issues to this worker's receipt claims;
/// a release must find its claim there.
fn work(
    machine: &Machine,
    (mi, wave, seed): (usize, usize, u64),
    per_wave: usize,
    keys: &Keys,
    claims: &mut BTreeMap<String, Json>,
) -> Done {
    let mut rng = Rng::new(seed + wave as u64 * 101 + mi as u64 * 907);
    let partitioned = mi == wave % 6 || mi == (wave + 2) % 6;
    let mut done = Done::default();
    for step in 0..per_wave {
        let args = command(&mut rng, (mi, wave, step), partitioned, keys);
        let run = run_json(machine, &args);
        let exit = run.returncode();
        let mut row = Row {
            wave,
            machine: mi,
            step,
            partitioned,
            args,
            exit,
            error: None,
        };
        if exit == 0 {
            done.record(&row, &run, claims);
        } else {
            let code = error_code(&run)
                .unwrap_or_else(|| panic!("unstructured error {row:?}: {}", run.stderr));
            assert!(ALLOWED.contains(&code.as_str()), "{row:?}: {}", run.stderr);
            row.error = Some(code);
        }
        done.rows.push(row);
    }
    done
}

/// The worker's command at `step`.
fn command(
    rng: &mut Rng,
    (mi, wave, step): (usize, usize, usize),
    partitioned: bool,
    keys: &Keys,
) -> Vec<String> {
    let action = rng.weighted(&ACTIONS);
    let key = rng.choice(&keys.editable).as_str();
    let tag = format!("{wave}/{mi}/{step}");
    match action {
        Action::Show => words(["show", rng.choice(&keys.all)]),
        Action::List => words(["list", "--all", "--limit", "0"]),
        Action::Ready => words(["ready"]),
        Action::Edit => {
            let priority = rng.below(5).to_string();
            let edit = *rng.choice(&["--add-label", "--remove-label"]);
            let label = format!("tag{}", rng.below(4));
            words([
                "update",
                key,
                "--notes",
                &tag,
                "--priority",
                &priority,
                edit,
                &label,
            ])
        }
        Action::Comment => words(["comments", "add", key, &format!("synthetic {tag}")]),
        Action::Create => words(["create", &format!("new {tag}")]),
        Action::Dep => {
            let op = *rng.choice(&["add", "remove"]);
            words(["dep", op, key, rng.choice(&keys.editable)])
        }
        Action::Status => {
            let status = *rng.choice(&["open", "closed", "deferred", "blocked"]);
            words(["update", key, "--status", status])
        }
        Action::Claim | Action::Release | Action::Takeover => {
            let key = rng.choice(&keys.owned).as_str();
            match action {
                _ if partitioned => words(["show", key]),
                Action::Claim => words(["claim", key]),
                Action::Release => words(["release", key]),
                _ => words(["takeover", key, "--reason", "concurrency audit"]),
            }
        }
        Action::Sync if partitioned => words(["info"]),
        Action::Sync => words(["sync"]),
    }
}

impl Done {
    /// What a successful command leaves: a new issue's key; for a claim or a takeover, a receipt
    /// of the claim it printed, which is the machine's claim on that issue from now on; for a
    /// release, a receipt of the claim it ended, which must be one of the machine's own.
    fn record(&mut self, row: &Row, run: &Run, claims: &mut BTreeMap<String, Json>) {
        let val = reply(run, row);
        let key = row.args.get(1).cloned().unwrap_or_default();
        match row.args[0].as_str() {
            "create" => self.newkeys.push(id(&val)),
            op @ ("claim" | "takeover") => {
                let held = val[0].get("claim").filter(|held| held.truthy());
                let held = held.unwrap_or_else(|| panic!("{row:?}: no claim in {val:?}"));
                self.receipts.push(Receipt::of(op, &key, held));
                claims.insert(key, held.clone());
            }
            "release" => {
                let held = claims.get(&key);
                let held = held.unwrap_or_else(|| {
                    panic!("released without own accepted claim receipt: {row:?}")
                });
                self.receipts.push(Receipt::of("release", &key, held));
            }
            _ => {}
        }
    }
}

/// The model's run: the fleet, the issues, the receipts and what happened so far.
struct Fleet {
    machines: Vec<Machine>,
    keys: Keys,
    /// Every receipt, wave after wave (`histories`).
    histories: Vec<Receipt>,
    /// Each machine's claims by issue, from its own receipts (`workerclaims`).
    claims: Vec<BTreeMap<String, Json>>,
    /// Each owned issue's last generation at a checkpoint (`generations`).
    generations: BTreeMap<String, i64>,
    operations: usize,
    counts: BTreeMap<String, usize>,
    errors: BTreeMap<String, usize>,
    started: Instant,
}

/// `run_concurrent_stress`: `waves` waves of `per_wave` commands from each machine, seeded from
/// `seed`, each wave checked; the run, once it has passed.
fn run_concurrent_stress(waves: usize, per_wave: usize, seed: u64) -> Fleet {
    let started = Instant::now();
    let sandbox = crate::sandbox();
    let machines = machines(&sandbox, "wave");
    let first = &machines[0];
    first.bead(["init", "--prefix", "wave"]);
    let create = |title: String| id(&first.bead(["create", &title]).expect("an issue"));
    let editable: Vec<String> = (0..24).map(|n| create(format!("editable {n}"))).collect();
    let owned: Vec<String> = (0..12).map(|n| create(format!("ownership {n}"))).collect();
    for machine in &machines {
        machine.bead(["sync"]);
    }
    let all = editable.iter().chain(&owned).cloned().collect();
    let mut fleet = Fleet {
        machines,
        keys: Keys {
            editable,
            owned,
            all,
        },
        histories: Vec::new(),
        claims: (0..6).map(|_| BTreeMap::new()).collect(),
        generations: BTreeMap::new(),
        operations: 0,
        counts: BTreeMap::new(),
        errors: BTreeMap::new(),
        started,
    };
    for wave in 0..waves {
        fleet.wave(wave, per_wave, seed);
        fleet.check(wave);
    }
    eprintln!(
        "passed: {} operations on 6 machines in {waves} waves of {per_wave}, seed {seed}, \
         {:.1} s; counts {:?}; domain errors {:?}",
        fleet.operations,
        started.elapsed().as_secs_f64(),
        fleet.counts,
        fleet.errors
    );
    fleet
}

impl Fleet {
    /// Wave `wave`: every machine's worker at once, a thread each, then what they did gathered in
    /// the machines' order.
    fn wave(&mut self, wave: usize, per_wave: usize, seed: u64) {
        let keys = &self.keys;
        let done: Vec<Done> = std::thread::scope(|scope| {
            let threads: Vec<_> = (self.machines.iter().zip(&mut self.claims))
                .enumerate()
                .map(|(mi, (machine, claims))| {
                    scope.spawn(move || work(machine, (mi, wave, seed), per_wave, keys, claims))
                })
                .collect();
            threads.into_iter().map(joined).collect()
        });
        for done in done {
            for row in &done.rows {
                *self.counts.entry(row.args[0].clone()).or_default() += 1;
                if let Some(code) = &row.error {
                    *self.errors.entry(code.clone()).or_default() += 1;
                }
            }
            self.operations += done.rows.len();
            self.keys.editable.extend(done.newkeys.iter().cloned());
            self.keys.all.extend(done.newkeys);
            self.histories.extend(done.receipts);
        }
    }

    /// Checks after wave `wave`.
    fn check(&mut self, wave: usize) {
        for machine in self.machines.iter().chain(&self.machines) {
            let r = machine.bead(["sync"]).expect("a sync report");
            let settled = r["waiting_events"].as_i64() == Some(0)
                && !r["forked_journals"].truthy()
                && r["integrity"].as_i64() == Some(0);
            assert!(settled, "wave {wave}: {} {r:?}", machine.name());
        }
        let exports = each(&self.machines, |_, machine| machine.export());
        let same = exports.iter().all(|export| *export == exports[0]);
        assert!(same, "export divergence, wave {wave}");
        let keys = exported_keys(&exports[0]);
        let details = each(&self.machines, |_, machine| details(machine, &keys));
        let same = details.iter().all(|d| sorted(d) == sorted(&details[0]));
        assert!(same, "detail divergence, wave {wave}");
        let expected = self.expected_claims();
        check_claims(wave, &details[0], &expected);
        let ready = expected_ready(wave, &details[0]);
        let events = each(&self.machines, |_, machine| settled(wave, machine, &ready));
        let accepted: BTreeSet<String> = (self.histories.iter())
            .filter(|r| !r.released())
            .map(|r| r.id.clone())
            .collect();
        let gone = missing(&accepted, &realized(&viewed(&self.machines[0])));
        assert!(gone.is_empty(), "accepted claim disappeared {gone:?}");
        let generation = self.generations.values().max().copied().unwrap_or(0);
        eprintln!(
            "checkpoint after wave {wave}: {} operations, {} issues, {} events, \
             {} accepted claim ids, max generation {generation} ({:.1} s)",
            self.operations,
            keys.len(),
            events.last().copied().unwrap_or(0),
            accepted.len(),
            self.started.elapsed().as_secs_f64(),
        );
    }

    /// The claim each owned issue must have, from the receipts: the claim of the highest
    /// generation, or none if that claim was released. One claim to a generation, and a
    /// generation that never goes back from one checkpoint to the next.
    fn expected_claims(&mut self) -> BTreeMap<String, Option<String>> {
        let mut expected = BTreeMap::new();
        for key in &self.keys.owned {
            let got: Vec<&Receipt> = self.histories.iter().filter(|r| r.key == *key).collect();
            let Some(top) = got.iter().map(|r| r.generation).max() else {
                expected.insert(key.clone(), None);
                continue;
            };
            let generation: Vec<&&Receipt> = got.iter().filter(|r| r.generation == top).collect();
            let claims: BTreeSet<&str> = (generation.iter())
                .filter(|r| !r.released())
                .map(|r| r.id.as_str())
                .collect();
            assert_eq!(
                claims.len(),
                1,
                "same generation multiple owners {key} {generation:?}"
            );
            let released = generation.iter().any(|r| r.released());
            let current = claims.first().map(|claim| (*claim).to_owned());
            expected.insert(key.clone(), current.filter(|_| !released));
            let last = self.generations.get(key).copied().unwrap_or(0);
            assert!(top >= last, "{key}: generation {top} after {last}");
            self.generations.insert(key.clone(), top);
        }
        expected
    }
}

/// Ten waves in which six machines, two of them partitioned, run fifty seeded commands each at
/// once: 3,000 commands, the fleet checked after every wave.
#[test]
#[ignore = "stress: randomised models"]
fn six_machine_3000_concurrent_commands_with_partitions() {
    let fleet = run_concurrent_stress(10, 50, 20_261_007);
    assert_eq!(fleet.operations, 3000);
}
