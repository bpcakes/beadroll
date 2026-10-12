//! Storage audit regressions for segments, torn tails, rotation, reader/writer races and initial
//! machine identity. Build-specific failpoints stop claims before publication, readers around
//! journal and pending reads, and identity creation before linking. The `pause-until` handshake
//! exposes each boundary; `@always` pauses every hit. Direct damage tests using the selected
//! binary are in `damaged`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use beadroll_store::glob_ending;
use beadroll_test_support::{Json, Machine, Run, Sandbox, code, parse_json};

use started::{Gate, Started, wait};

mod damaged;
mod started;

/// How a test makes its machines: `Machine::new`, which runs the bead under test, or
/// `crate::rust_only` for a test that stops bead at a failpoint.
type Make = fn(&Sandbox, &str) -> Machine;

/// The class's `setUp`: machine `storage-a`, made by `make`, has initialised the tracker `ex` and
/// created the issue the tests work on, `storage survivor`. Returns the machine and the issue's id.
fn set_up(sandbox: &Sandbox, make: Make) -> (Machine, String) {
    let a = make(sandbox, "storage-a");
    a.bead(["init", "--prefix", "ex"]);
    let created = a.bead(["create", "storage survivor"]).expect("the issue");
    let key = created["id"].as_str().expect("the issue's id").to_owned();
    (a, key)
}

/// `own`: this machine's id, as `bead info` reports it.
fn own(machine: &Machine) -> String {
    let info = machine.bead(["info"]).expect("bead info prints a record");
    info["machine"]
        .as_str()
        .expect("the machine's id")
        .to_owned()
}

/// `two`: a publishes its tracker, and a second machine, `storage-b`, made by `make`, syncs it.
fn two(sandbox: &Sandbox, a: &Machine, make: Make) -> Machine {
    a.bead(["sync"]);
    let b = make(sandbox, "storage-b");
    b.bead(["sync"]);
    b
}

/// The segments of the machine's own journal, in order:
/// `sorted((store / "events" / own).glob("*.jsonl"))`.
fn segments(machine: &Machine) -> Vec<PathBuf> {
    let folder = machine.store().join("events").join(own(machine));
    glob_ending(&folder, ".jsonl")
}

/// The machine's own journal, each segment's bytes by its path:
/// `{p: p.read_bytes() for p in (store / "events" / own).glob("*.jsonl")}`.
fn journal(machine: &Machine) -> BTreeMap<PathBuf, Vec<u8>> {
    segments(machine)
        .into_iter()
        .map(|path| {
            let bytes = std::fs::read(&path).expect("a segment");
            (path, bytes)
        })
        .collect()
}

/// The machine's pending attempts: `sorted((store / "pending").glob("*.json"))`.
fn pending(machine: &Machine) -> Vec<PathBuf> {
    glob_ending(&machine.store().join("pending"), ".json")
}

/// The events of the attempt file `path`, one JSON document a line.
fn events(path: &Path) -> Vec<Json> {
    let text = std::fs::read_to_string(path).expect("an attempt file");
    let lines = text.lines().map(|line| parse_json(line).expect("an event"));
    lines.collect()
}

/// The member `key` of `value`; absence fails the test.
fn member<'v>(value: &'v Json, key: &str) -> &'v Json {
    value
        .get(key)
        .unwrap_or_else(|| panic!("no {key:?} in {value:?}"))
}

/// The claim `machine` takes on `key`: `machine.bead("claim", key)[0]["claim"]`.
fn claim(machine: &Machine, key: &str) -> Json {
    let claimed = machine.bead(["claim", key]).expect("the claimed issue");
    member(&claimed[0], "claim").clone()
}

/// The first issue a `show --json` printed.
fn shown(run: &Run) -> Json {
    let printed = parse_json(&run.stdout)
        .unwrap_or_else(|error| panic!("bead printed no JSON ({error}): {}", run.stdout));
    printed[0].clone()
}

/// The temporary files an identity is written to before it is linked into place:
/// `home.glob(".replica.*.tmp")`.
fn identity_temporaries(home: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(home) else {
        return Vec::new();
    };
    let named = |path: &Path| {
        let name = path.file_name().expect("a name").to_string_lossy();
        name.starts_with(".replica.") && name.ends_with(".tmp")
    };
    let paths = entries.map(|entry| entry.expect("a directory entry").path());
    paths.filter(|path| named(path)).collect()
}

/// A refused claiming update leaves the journal byte-identical before, during and after the
/// attempt; a reader must see no partial state. Pause the update at its first `publish.before`
/// and the reader at each `journal.after-enumerate`, between listing and reading segments.
#[test]
fn refused_claiming_update_leaves_the_journal_untouched_under_a_reader() {
    let sandbox = crate::sandbox();
    let (a, key) = set_up(&sandbox, crate::rust_only);
    let b = two(&sandbox, &a, crate::rust_only);
    let (publish, read) = (Gate::new(&sandbox, "publish"), Gate::new(&sandbox, "read"));
    let journal_before = journal(&a);
    let update = [
        "update",
        &key,
        "--claim",
        "--title",
        "refused title",
        "--add-label",
        "refused",
        "--json",
    ];
    let claiming = a
        .cmd(update)
        .env("BEAD_ROTATE_BYTES", "1")
        .env("BEAD_FAILPOINTS", publish.at("publish.before"));
    let mut claimer = Started::start(claiming);
    wait(&publish.ready(), &mut claimer);
    let attempts = pending(&a);
    assert_eq!(attempts.len(), 1, "the attempt is one file");
    let ops: Vec<Json> = events(&attempts[0])
        .iter()
        .map(|event| member(event, "op").clone())
        .collect();
    assert_eq!(
        ops,
        ["claim", "set", "label_add"].map(Json::from),
        "claim, set and label_add form one attempt"
    );
    assert_eq!(journal(&a), journal_before);
    let pausing = read.at("journal.after-enumerate@always");
    let reading = a.cmd(["show", &key, "--json"]);
    let mut reader = Started::start(reading.env("BEAD_FAILPOINTS", pausing));
    wait(&read.ready(), &mut reader);
    let accepted = claim(&b, &key);
    publish.open();
    let claimed = claimer.communicate(Duration::from_secs(15));
    assert_eq!(claimed.returncode(), 1, "{}", claimed.stderr);
    assert_eq!(code(&claimed), "ALREADY_CLAIMED");
    assert_eq!(
        pending(&a),
        Vec::<PathBuf>::new(),
        "a refused attempt leaves nothing behind"
    );
    assert_eq!(journal(&a), journal_before);
    read.open();
    let read_out = reader.communicate(Duration::from_secs(10));
    assert_eq!(read_out.returncode(), 0, "{}", read_out.stderr);
    let shown = shown(&read_out);
    assert_eq!(shown["title"].as_str(), Some("storage survivor"));
    let labels = shown.get("labels").and_then(Json::as_array);
    let labels = labels.unwrap_or_default();
    assert!(!labels.contains(&Json::from("refused")), "{labels:?}");
    let seen = shown.get("claim");
    assert!(
        seen.is_none() || seen == Some(&accepted),
        "the reader showed something other than nothing or the accepted claim: {seen:?}"
    );
    a.bead(["sync"]);
    assert_eq!(member(&a.show(&key), "claim"), &accepted);
    assert_eq!(a.show(&key)["title"].as_str(), Some("storage survivor"));
}

/// An unlocked journal/pending snapshot must not promote a refused claim. Pause the claim at its
/// first `publish.before` and the reader at every `view.before-load-pending`, after replay and
/// before pending attempts are loaded.
#[test]
fn reader_never_confirms_a_claim_whose_push_was_rejected() {
    let sandbox = crate::sandbox();
    let (a, key) = set_up(&sandbox, crate::rust_only);
    let b = two(&sandbox, &a, crate::rust_only);
    let (publish, read) = (Gate::new(&sandbox, "claim"), Gate::new(&sandbox, "view"));
    let claiming = a.cmd(["claim", &key, "--json"]);
    let mut claimer = Started::start(claiming.env("BEAD_FAILPOINTS", publish.at("publish.before")));
    wait(&publish.ready(), &mut claimer);
    let attempt = pending(&a).into_iter().next();
    let attempt = attempt.expect("the claim's pending attempt");
    let failed_id = member(&events(&attempt)[0], "id").clone();
    let pausing = read.at("view.before-load-pending@always");
    let reading = a.cmd(["show", &key, "--json"]);
    let mut reader = Started::start(reading.env("BEAD_FAILPOINTS", pausing));
    wait(&read.ready(), &mut reader);
    let accepted = claim(&b, &key);
    publish.open();
    let claimed = claimer.communicate(Duration::from_secs(15));
    assert_eq!(claimed.returncode(), 1, "{}", claimed.stderr);
    assert_eq!(code(&claimed), "ALREADY_CLAIMED");
    assert_eq!(pending(&a), Vec::<PathBuf>::new());
    assert_ne!(member(&accepted, "id"), &failed_id);
    read.open();
    let read_out = reader.communicate(Duration::from_secs(10));
    assert_eq!(read_out.returncode(), 0, "{}", read_out.stderr);
    let shown = shown(&read_out);
    let shown_id = shown.get("claim").and_then(|claim| claim.get("id"));
    assert_ne!(
        shown_id,
        Some(&failed_id),
        "reader promoted an online-rejected claim to a confirmed owner: {}",
        read_out.stdout
    );
}

/// Separate workspace locks share one newly created replica identity. Pause the first process at
/// every `identity.before-link`, after writing the temporary identity but before linking it into
/// place, then run a second bead normally.
#[test]
fn machine_identity_first_use_is_atomic_across_workspaces() {
    let sandbox = crate::sandbox();
    let _ = set_up(&sandbox, crate::rust_only);
    let fresh = crate::rust_only(&sandbox, "identity-fresh");
    let gate = Gate::new(&sandbox, "identity");
    // offline: these are workspaces named beside the tracker `set_up` published, which `init`
    // online refuses to start a second tracker against; the machine's identity is what is tested
    let init = |workspace: &str| {
        let init = fresh.cmd(["init", "--offline", "--json"]);
        init.env("BEAD_WORKSPACE", workspace)
    };
    let pausing =
        init("identity-one").env("BEAD_FAILPOINTS", gate.at("identity.before-link@always"));
    let mut first = Started::start(pausing);
    wait(&gate.ready(), &mut first);
    let mut second = Started::start(init("identity-two"));
    // a correct implementation may wait for the creator
    let second_done = second.within(Duration::from_secs(2));
    gate.open();
    let second_out = second_done.unwrap_or_else(|| second.communicate(Duration::from_secs(10)));
    let first_out = first.communicate(Duration::from_secs(10));
    assert_eq!(first_out.returncode(), 0, "{}", first_out.stderr);
    assert_eq!(
        second_out.returncode(),
        0,
        "another workspace observed the half-written global machine id:\n{}",
        second_out.stderr
    );
    let info = |workspace: &str| {
        let info = fresh.cmd(["info", "--json"]);
        info.env("BEAD_WORKSPACE", workspace).run()
    };
    let (one, two) = (info("identity-one"), info("identity-two"));
    assert_eq!(
        (one.returncode(), two.returncode()),
        (0, 0),
        "{}{}",
        one.stderr,
        two.stderr
    );
    let machine = |run: &Run| {
        let info = parse_json(&run.stdout).expect("info prints JSON");
        member(&info, "machine").clone()
    };
    assert_eq!(
        machine(&one),
        machine(&two),
        "the two workspaces ended up with different machine identities"
    );
}

/// SIGKILL before linking the identity must leave it absent or complete and allow later writes.
/// Pause at each `identity.before-link` and kill the process group there.
#[test]
fn machine_identity_can_recover_from_first_use_crash() {
    let sandbox = crate::sandbox();
    let _ = set_up(&sandbox, crate::rust_only);
    let fresh = crate::rust_only(&sandbox, "identity-killed");
    let gate = Gate::new(&sandbox, "identity-killed");
    // offline, as in the test above: a workspace named beside the published tracker
    let init = || {
        let init = fresh.cmd(["init", "--offline", "--json"]);
        init.env("BEAD_WORKSPACE", "identity-crash")
    };
    let pausing = init().env("BEAD_FAILPOINTS", gate.at("identity.before-link@always"));
    let mut process = Started::start(pausing);
    wait(&gate.ready(), &mut process);
    process.kill();
    let killed = process.communicate(Duration::from_secs(10));
    assert_eq!(killed.returncode(), -9, "{}", killed.stderr);
    assert!(
        !fresh.home().join("replica.json").exists(),
        "a half-made identity became visible"
    );
    assert!(
        !identity_temporaries(fresh.home()).is_empty(),
        "the interrupted write should have been in progress"
    );
    let retry = init().run();
    assert_eq!(
        retry.returncode(),
        0,
        "first-use crash left a permanent unreadable machine identity:\n{}",
        retry.stderr
    );
}
