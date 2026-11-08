//! A parallel first download preserves journal bytes and retries interrupted installation.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use beadroll_test_support::{Machine, Sandbox};

fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, path: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(path).into_iter().flatten() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(root, &path, out);
            } else if path
                .extension()
                .is_some_and(|e| e == "json" || e == "jsonl")
            {
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

#[test]
fn bootstrap_recovers_at_batch_boundaries_and_after_write_error() {
    let sandbox = crate::sandbox();
    let a = seeded(&sandbox);
    let expected = files(&a.store().join("events"));
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
            assert!(
                partial
                    .iter()
                    .all(|(path, bytes)| expected.get(path) == Some(bytes))
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
    assert!(run.stderr.contains("IO"), "{}", run.stderr);
    c.bead(["sync"]);
    assert_eq!(files(&c.store().join("events")), expected);
}
