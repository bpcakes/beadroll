//! A parallel first download preserves journal bytes and retries interrupted installation.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use beadroll_test_support::{Machine, Sandbox, code, git};

use crate::hardening::gate::{Gate, beside};

fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    stored_files(root)
        .into_iter()
        .filter(|(path, _)| {
            path.extension()
                .is_some_and(|e| e == "json" || e == "jsonl")
        })
        .collect()
}

fn stored_files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, path: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(path).into_iter().flatten() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(root, &path, out);
            } else {
                out.insert(
                    path.strip_prefix(root).unwrap().to_owned(),
                    std::fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    visit(root, root, &mut out);
    out
}

/// The fixture's journal paths in the remote's order, independently of the local map's order.
fn listing(sandbox: &Sandbox) -> Vec<PathBuf> {
    git(
        [
            "ls-tree",
            "-r",
            "--name-only",
            "-z",
            "refs/beads/journal",
            "--",
            "events/",
        ],
        Some(sandbox.origin()),
    )
    .split('\0')
    .filter(|path| !path.is_empty())
    .map(|path| Path::new(path).strip_prefix("events").unwrap().to_owned())
    .collect()
}

fn prefix(
    expected: &BTreeMap<PathBuf, Vec<u8>>,
    order: &[PathBuf],
    count: usize,
) -> BTreeMap<PathBuf, Vec<u8>> {
    order
        .iter()
        .take(count)
        .map(|path| (path.clone(), expected[path].clone()))
        .collect()
}

/// A paused preparation owns this temporary until its gate opens.
fn prepared_bytes(root: &Path, target: &Path) -> Vec<u8> {
    let name = format!(".{}.", target.file_name().unwrap().to_str().unwrap());
    let temporary = std::fs::read_dir(root.join(target.parent().unwrap()))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with(&name)
        })
        .expect("the flushed temporary exists");
    std::fs::read(temporary).unwrap()
}

fn seeded(sandbox: &Sandbox) -> Machine {
    let a = crate::rust_only(sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let key = a.bead(["create", "work"]).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    for _ in 0..10 {
        a.bead(["claim", &key]);
        a.bead(["release", &key]);
    }
    assert!(files(&a.store().join("events")).len() > 16);
    let machines = a.bead(["machines"]).unwrap();
    let id = machines.as_array().unwrap()[0]["machine"].as_str().unwrap();
    a.bead(["machines", "--name", &format!("{id}={id}")]);
    a
}

/// AP03: every interrupted state is exactly the listing prefix, including before retry.
#[test]
fn bootstrap_recovers_at_batch_boundaries_and_after_write_error() {
    let sandbox = crate::sandbox();
    let a = seeded(&sandbox);
    let expected = files(&a.store().join("events"));
    let order = listing(&sandbox);
    assert_eq!(order.len(), expected.len());
    let export = a.run(["export"]).stdout;
    let log = a.run(["log", "--limit", "0", "--json"]).stdout;
    for (n, point) in ["atomic-write:events", "atomic-written:events"]
        .into_iter()
        .enumerate()
    {
        for boundary in [1, 8, 9, expected.len()] {
            let b = crate::rust_only(&sandbox, &format!("b{n}-{boundary}"));
            let fault = format!("{point}@nth({boundary})=exit(98)");
            let run = b
                .cmd(["sync", "--json"])
                .env("BEAD_FAILPOINTS", &fault)
                .run();
            assert_eq!(run.returncode(), 98, "{fault}: {}", run.stderr);
            let partial = files(&b.store().join("events"));
            assert_eq!(
                partial,
                prefix(&expected, &order, boundary - 1 + n),
                "{fault}"
            );
            b.bead(["sync"]);
            assert_eq!(files(&b.store().join("events")), expected);
            assert_eq!(b.run(["export"]).stdout, export);
            assert_eq!(b.run(["log", "--limit", "0", "--json"]).stdout, log);
        }
    }
    let c = crate::rust_only(&sandbox, "c");
    let run = c
        .cmd(["sync", "--json"])
        .env(
            "BEAD_FAILPOINTS",
            "atomic-write:events@nth(3)=errno(ENOSPC)",
        )
        .run();
    assert_eq!(run.returncode(), 1, "{}", run.stderr);
    assert_eq!(code(&run), "IO", "{}", run.stderr);
    assert_eq!(
        files(&c.store().join("events")),
        prefix(&expected, &order, 2)
    );
    assert_eq!(
        stored_files(&c.store().join("events")),
        files(&c.store().join("events"))
    );
    c.bead(["sync"]);
    assert_eq!(files(&c.store().join("events")), expected);
}

/// AP02: a preparation error at a known listing slot retains only earlier installations,
/// discards the successful later preparations, and can be retried to the same journal.
#[test]
fn bootstrap_discards_prepared_siblings_after_a_flush_error_and_retries() {
    let sandbox = crate::sandbox();
    let a = seeded(&sandbox);
    let expected = files(&a.store().join("events"));
    let order = listing(&sandbox);
    let export = a.run(["export"]).stdout;
    let log = a.run(["log", "--limit", "0", "--json"]).stdout;
    let b = crate::rust_only(&sandbox, "failed-preparation");
    let gate = Gate::new(&sandbox, "later-file-flushed");
    // Path matching chooses the third slot regardless of worker scheduling. The last worker
    // pauses after a successful fsync, proving a later sibling is prepared before return.
    let fault = format!(
        "atomic-flush:events/{}=errno(ENOSPC);atomic-flushed:events/{}={}",
        order[2].display(),
        order[7].display(),
        gate.action(),
    );
    let (run, reached) = beside(
        || {
            b.cmd(["sync", "--json"])
                .env("BEAD_FAILPOINTS", &fault)
                .run()
        },
        |finished| {
            gate.meet(finished, || {
                let events = b.store().join("events");
                assert!(
                    order.iter().all(|path| !events.join(path).exists()),
                    "nothing installs before every worker joins"
                );
                assert_eq!(prepared_bytes(&events, &order[7]), expected[&order[7]]);
            })
        },
    );
    assert!(
        reached.is_some(),
        "the later preparation must finish its flush: {}",
        run.stderr
    );
    assert_eq!(run.returncode(), 1, "{}", run.stderr);
    assert_eq!(code(&run), "IO", "{}", run.stderr);
    // Comparing every file also detects abandoned temporaries after ordinary error unwinding.
    let events = b.store().join("events");
    assert_eq!(stored_files(&events), prefix(&expected, &order, 2));
    b.bead(["sync"]);
    assert_eq!(stored_files(&events), expected);
    assert_eq!(b.run(["export"]).stdout, export);
    assert_eq!(b.run(["log", "--limit", "0", "--json"]).stdout, log);
}
