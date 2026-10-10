//! `tests/test_stress.py`, ported: two randomised models in which six machines share one remote
//! and run thousands of commands, checked as they go against a reference model or invariants.
//! They are slow, so they are ignored by default; `cargo nextest run -p beadroll --run-ignored only
//! -E 'test(/^stress::/)'` runs them.
//!
//! How the Python mechanisms are ported:
//! - Partitions. Python patches nothing in bead: a partitioned machine withholds sync, running
//!   `info` instead of `sync` and `show` instead of a command that needs the remote (in the
//!   reference model, `claim --offline` instead of `claim`), and so do these ports. They need no
//!   failpoint, and nothing waits less than in Python: the backoff between rounds is bead's own.
//! - The in-process reads. Python read a machine's store with its own `load_journal`, `replay` and
//!   `view`, `BEAD_HOME` swapped for the machine's; these ports read it with beadroll-store's and
//!   beadroll-core's ([`replayed`], [`viewed`]), as `docs/RUST-PORT.md` §4.8 has it. Their machines
//!   run the Rust `bead` whatever `BEAD_UNDER_TEST` says ([`crate::rust_only`]), so that, as in
//!   Python, the implementation that reads a store in-process is the one that wrote it.
//! - The mixed fleet (§4.2, item 4). The first model also runs with its six machines alternating
//!   Python's `bead` and the Rust one ([`mixed_machines`]), where every full check requires the
//!   two implementations to agree; there Rust's in-process reads take the Python machines' stores
//!   too, which they must read as the Rust machines' own.
//! - Randomness. Python drew its choices from `random.Random(seed)`; these ports draw them, in the
//!   same order, from a port of it ([`Rng`], CPython's Mersenne Twister), so a seed makes the
//!   choices Python's made. Nothing a command answers changes the reference model's draws but
//!   the issues it creates, so the model runs Python's very commands; a wave's workers make
//!   Python's draws too, in an interleaving of their own.
//! - Concurrency. Where Python ran commands at once (a wave's six workers), these ports do too;
//!   where it ran them one after another, so do they, except that the per-machine reads of a
//!   checkpoint, which touch only the machine's own store, run a thread per machine ([`each`]).

mod concurrent_reaudit;
mod python_random;
mod root_audit_stress;

pub(crate) use python_random::Rng;

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::Path;
use std::sync::Arc;

use beadroll_core::json::{Format, Object, write};
use beadroll_core::replay::replay;
use beadroll_core::state::State;
use beadroll_store::{Settings, Workspace, load_journal, view};
use beadroll_test_support::{Json, Machine, Run, Sandbox, parse_json};

/// How a model makes its fleet: [`machines`] or [`mixed_machines`].
pub(crate) type Fleet = fn(&Sandbox, &str) -> Vec<Machine>;

/// The machines of a model: `NAME0` to `NAME5`, each running the Rust `bead`.
pub(crate) fn machines(sandbox: &Sandbox, name: &str) -> Vec<Machine> {
    (0..6)
        .map(|n| crate::rust_only(sandbox, &format!("{name}{n}")))
        .collect()
}

/// The machines of a mixed fleet: `NAME0` to `NAME5`, running Python's `bead` (the checkout's
/// launcher) and the Rust one in turn, Python's first, whatever `BEAD_UNDER_TEST` and
/// `BEAD_MIXED_WITH` say.
pub(crate) fn mixed_machines(sandbox: &Sandbox, name: &str) -> Vec<Machine> {
    let beads = [crate::mixed::python(), crate::mixed::rust()];
    (0..6)
        .map(|n| Machine::new(sandbox, &format!("{name}{n}")).with_bead(&beads[n % 2]))
        .collect()
}

/// `parts` as owned words: a command line for [`run_json`].
pub(crate) fn words<const N: usize>(parts: [&str; N]) -> Vec<String> {
    parts.map(str::to_owned).to_vec()
}

/// `bead ARGS --json` from `machine`, as the models' `m.run(*args, "--json")`.
pub(crate) fn run_json(machine: &Machine, args: &[String]) -> Run {
    machine.run(args.iter().map(String::as_str).chain(["--json"]))
}

/// The error code a failed `bead ... --json` reported, or `None` when its standard error is not
/// the structured error the models require.
pub(crate) fn error_code(run: &Run) -> Option<String> {
    let document = parse_json(&run.stderr).ok()?;
    document["error"]["code"].as_str().map(str::to_owned)
}

/// What a successful `bead ... --json` printed; output that is not JSON fails the model.
pub(crate) fn reply(run: &Run, context: &dyn std::fmt::Debug) -> Json {
    parse_json(&run.stdout).unwrap_or_else(|error| {
        panic!(
            "{context:?}: bead printed no JSON ({error}): {}",
            run.stdout
        )
    })
}

/// `value["id"]`, a string.
pub(crate) fn id(value: &Json) -> String {
    let id = value["id"].as_str();
    id.unwrap_or_else(|| panic!("no id in {value:?}"))
        .to_owned()
}

/// `json.dumps(value, sort_keys=True)`: what two machines' answers are compared by.
pub(crate) fn sorted(value: &Json) -> String {
    write(value, Format::SORTED_ASCII).expect("bead's JSON writes back")
}

/// `"status" in issue.get("conflicts", {})`: whether the issue's status is disputed.
pub(crate) fn status_disputed(issue: &Json) -> bool {
    issue
        .get("conflicts")
        .and_then(|c| c.get("status"))
        .is_some()
}

/// `bead show KEYS --json` from `machine`: every issue in `keys`, in full.
pub(crate) fn details(machine: &Machine, keys: &[String]) -> Json {
    let args = std::iter::once("show").chain(keys.iter().map(String::as_str));
    machine.bead(args).expect("bead show prints a list")
}

/// `f(n, machine)` for every machine at once, a thread each: the answers in the machines' order.
/// A thread's panic is the caller's, its message kept.
pub(crate) fn each<'m, R, F>(machines: &'m [Machine], f: F) -> Vec<R>
where
    R: Send,
    F: Fn(usize, &'m Machine) -> R + Sync,
{
    std::thread::scope(|scope| {
        let f = &f;
        let threads: Vec<_> = machines
            .iter()
            .enumerate()
            .map(|(n, machine)| scope.spawn(move || f(n, machine)))
            .collect();
        threads.into_iter().map(joined).collect()
    })
}

/// What a scoped thread returned; its panic, re-raised with its own message, if it panicked.
pub(crate) fn joined<R>(thread: std::thread::ScopedJoinHandle<'_, R>) -> R {
    thread
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
}

/// The workspace `machine`'s store holds, opened in-process as Python's models opened it:
/// `Workspace(m.store().name, None, None, {})` with `BEAD_HOME` set to the machine's.
fn workspace(machine: &Machine) -> Workspace {
    let settings = Settings::from_process(
        machine.env(None, "codex"),
        Some(machine.checkout().to_path_buf()),
        std::env::var_os("HOME").unwrap_or_default(),
        std::env::temp_dir(),
        &|_| None,
    )
    .expect("the machine's settings");
    let store = machine.store();
    let wid = store.file_name().and_then(OsStr::to_str);
    let wid = wid.unwrap_or_else(|| panic!("{} names no workspace", store.display()));
    Workspace::new(Arc::new(settings), wid, None, None, Object::new()).expect("the workspace")
}

/// Python's `replay(load_journal(ws))` over `machine`'s store: the confirmed state.
pub(crate) fn replayed(machine: &Machine) -> State {
    let journal = load_journal(&workspace(machine)).expect("the machine's journal");
    replay(journal).expect("the machine's journal replays")
}

/// Python's `view(ws)` over `machine`'s store: the confirmed state, with the machine's own
/// unconfirmed attempts listed on their issues.
pub(crate) fn viewed(machine: &Machine) -> State {
    view(&workspace(machine)).expect("the machine's view")
}

/// Every claim `state` holds or has held: the ids of each issue's current claim and of its
/// `claim_history`.
pub(crate) fn realized(state: &State) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    for issue in state.issues() {
        ids.extend(issue.claim_history().iter().map(|e| e.claim.id.to_string()));
        ids.extend(issue.claim().map(|claim| claim.id.to_string()));
    }
    ids
}

/// The accepted claims `realized` lacks.
pub(crate) fn missing<'a>(
    accepted: &'a BTreeSet<String>,
    realized: &BTreeSet<String>,
) -> Vec<&'a String> {
    (accepted.iter())
        .filter(|claim| !realized.contains(*claim))
        .collect()
}

/// `path` read as JSON, when the file exists.
pub(crate) fn json_file(path: &Path) -> Option<Json> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => panic!("{}: {error}", path.display()),
    };
    Some(parse_json(&text).unwrap_or_else(|error| panic!("{}: {error}", path.display())))
}
