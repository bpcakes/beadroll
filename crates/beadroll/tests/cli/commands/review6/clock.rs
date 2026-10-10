//! F14 and a verified property: bead's clock is wrong. The Python tests run bead through a driver
//! that replaces `time.time_ns` for the whole run; these ports are `rust_only` and set the Rust
//! `bead`'s clock with the `clock` failpoint instead (`docs/RUST-PORT.md` §4.5), at every reading
//! (`clock@always`): `at(NS)` sets it to NS nanoseconds since the epoch, `offset(±DUR)` shifts it.

use std::collections::BTreeSet;

use beadroll_test_support::{Json, Machine, Sandbox};

use super::id;

/// The clock-skew test's clock: 48 hours behind at every reading.
const BEHIND: &str = "clock@always=offset(-48h)";

/// 47 hours in milliseconds: the 48 hours the clock is set back, less an hour for the time that
/// passes between two edits of the clock-skew test.
const NEARLY_48_HOURS: i64 = 47 * 3600 * 1000;

/// `self.machine(name)` for a `rust_only` test: a machine `name` that runs the Rust `bead`, its
/// tracker initialised with the prefix `ex`.
fn machine(sandbox: &Sandbox, name: &str) -> Machine {
    let machine = crate::rust_only(sandbox, name);
    machine.bead(["init", "--prefix", "ex"]);
    machine
}

/// The clock reading of each `set` event on `key`, in the order `bead log` lists them: the
/// milliseconds since the epoch that lead its id (a UUIDv7).
fn set_stamps(machine: &Machine, key: &str) -> Vec<i64> {
    let log = machine.bead(["log", key, "--limit", "0"]).expect("a log");
    let rows = log.as_array().expect("bead log prints a list");
    let sets = rows.iter().filter(|row| row["op"].as_str() == Some("set"));
    sets.map(|row| {
        let id = row["event"].as_str().expect("an event id").replace('-', "");
        i64::from_str_radix(&id[..12], 16).expect("a UUIDv7")
    })
    .collect()
}

/// F14: a clock past year 2100 does not turn create into a KeyError traceback.
///
/// Python's driver sets `time.time_ns` to 4,429,000,000,000,000,000 nanoseconds (the year 2110)
/// for the whole run: `clock@always=at(4429000000000000000)`.
#[test]
fn f14_future_clock_create_is_a_structured_error() {
    let sandbox = crate::sandbox();
    let machine = machine(&sandbox, "f14");
    let created = machine
        .cmd(["create", "future", "--json"])
        .env("BEAD_FAILPOINTS", "clock@always=at(4429000000000000000)")
        .run();
    assert!(!created.crashed(), "{}", created.stderr);
    assert_ne!(created.returncode(), 0, "{}", created.stdout);
    let listed = machine.bead(["list", "--all", "--limit", "0"]);
    let listed = listed.expect("bead list prints a record");
    let issues = listed["issues"].as_array().expect("a list of issues");
    let titles: Vec<&str> = issues
        .iter()
        .map(|issue| issue["title"].as_str().expect("a title"))
        .collect();
    assert!(!titles.contains(&"future"), "{titles:?}");
}

/// Verified (no finding): a write that has seen another still settles when its clock is behind.
///
/// Python's driver runs bead with `time.time_ns` 48 hours behind for the whole run:
/// `clock@always=offset(-48h)`, on the two updates machine `clock-future` makes through it. Since
/// causality would decide both registers the same way with the clock right, the port also shows
/// that it was not: each of those updates is stamped more than 47 hours before the edit it follows
/// or races.
#[test]
fn clock_skew_does_not_override_causality() {
    let sandbox = crate::sandbox();
    let behind = |machine: &Machine, key: &str, title: &str| {
        let update = machine.cmd(["update", key, "--title", title, "--json"]);
        update.env("BEAD_FAILPOINTS", BEHIND).run()
    };
    let past = machine(&sandbox, "clock-past");
    let key = id(past.bead(["create", "base"]));
    past.bead(["sync"]);
    let future = crate::rust_only(&sandbox, "clock-future");
    future.bead(["sync"]);
    past.bead(["update", &key, "--title", "from-a"]);
    past.bead(["sync"]);
    future.bead(["sync"]);
    let updated = behind(&future, &key, "from-b-behind");
    assert_eq!(updated.returncode(), 0, "{}", updated.stderr);
    future.bead(["sync"]);
    past.bead(["sync"]);
    let shown = past.show(&key);
    assert_eq!(shown["title"].as_str(), Some("from-b-behind"));
    assert!(shown.get("conflicts").is_none(), "{shown:?}");
    let stamps = set_stamps(&past, &key);
    assert!(
        matches!(stamps[..], [seen, behind] if seen - behind > NEARLY_48_HOURS),
        "{stamps:?}"
    );

    let key2 = id(past.bead(["create", "race"]));
    past.bead(["sync"]);
    future.bead(["sync"]);
    past.bead(["update", &key2, "--title", "alpha"]);
    let other = behind(&future, &key2, "beta");
    assert_eq!(other.returncode(), 0, "{}", other.stderr);
    past.bead(["sync"]);
    future.bead(["sync"]);
    past.bead(["sync"]);
    let raced = past.show(&key2);
    let titles = raced.get("conflicts").and_then(|c| c.get("title"));
    let titles = titles.and_then(Json::as_array);
    let titles = titles.unwrap_or_else(|| panic!("no title conflict in {raced:?}"));
    let values: BTreeSet<&str> = titles
        .iter()
        .map(|item| item["value"].as_str().expect("a title"))
        .collect();
    assert_eq!(values, BTreeSet::from(["alpha", "beta"]));
    let stamps = set_stamps(&past, &key2);
    assert!(
        matches!(stamps[..], [one, two] if (one - two).abs() > NEARLY_48_HOURS),
        "{stamps:?}"
    );
}
