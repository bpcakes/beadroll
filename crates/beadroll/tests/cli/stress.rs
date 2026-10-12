//! Two slow six-machine stress models run thousands of commands against one remote and check
//! reference state or invariants. Run them with `cargo nextest run -p beadroll --run-ignored only
//! -E 'test(/^stress::/)'`.
//!
//! Soft partitions replace selected remote operations with reads or offline claims. Backoff
//! remains the protocol's own. Checks use [`replayed`] and [`viewed`] to read each machine's
//! store, so machines run [`crate::rust_only`]. [`Rng`] preserves established seeded command
//! sequences. The reference model is sequential; each wave of the concurrency model has six
//! workers. Independent checkpoint reads run one thread per machine ([`each`]).

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

/// The machines of a model: `NAME0` to `NAME5`, each running the Rust `bead`.
pub(crate) fn machines(sandbox: &Sandbox, name: &str) -> Vec<Machine> {
    (0..6)
        .map(|n| crate::rust_only(sandbox, &format!("{name}{n}")))
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

/// Sorted-key JSON used to compare machine results.
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

/// Opens the machine's workspace in-process with that machine's home and store id.
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

/// The confirmed state from replaying the machine's journal.
pub(crate) fn replayed(machine: &Machine) -> State {
    let journal = load_journal(&workspace(machine)).expect("the machine's journal");
    replay(journal).expect("the machine's journal replays")
}

/// The confirmed state with the machine's unconfirmed attempts listed on their issues.
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
