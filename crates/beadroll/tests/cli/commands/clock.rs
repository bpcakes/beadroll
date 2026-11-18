//! F14 and a verified property: an incorrect clock. These `rust_only` tests set the `clock`
//! failpoint at every reading (`clock@always`). `at(NS)` sets nanoseconds since the epoch;
//! `offset(±DUR)` shifts the clock (`crates/beadroll-store/src/failpoint.rs`).

use std::collections::BTreeSet;

use beadroll_test_support::{Json, Machine, Sandbox, printed_id, reported_code};

use super::fixtures::set_stamps;

/// The clock-skew test's clock: 48 hours behind at every reading.
const BEHIND: &str = "clock@always=offset(-48h)";

/// 47 hours in milliseconds: the 48 hours the clock is set back, less an hour for the time that
/// passes between two edits of the clock-skew test.
const NEARLY_48_HOURS: i64 = 47 * 3600 * 1000;

/// A machine `name` that runs the Rust `bead`, its
/// tracker initialised with the prefix `ex`.
fn machine(sandbox: &Sandbox, name: &str) -> Machine {
    let machine = crate::rust_only(sandbox, name);
    machine.bead(["init", "--prefix", "ex"]);
    machine
}

/// There is no history boundary: work written offline six months ago (by the clock, set back 180
/// days) merges on the next sync, while the other machine has kept working, and nothing is refused
/// for its age.
#[test]
fn work_written_offline_months_ago_still_merges() {
    let sandbox = crate::sandbox();
    let away = machine(&sandbox, "clock-away");
    let key = printed_id(away.bead(["create", "shared"]));
    away.bead(["sync"]);
    let here = crate::rust_only(&sandbox, "clock-here");
    here.bead(["sync"]);
    let old = away
        .cmd([
            "comments",
            "add",
            &key,
            "written months ago",
            "--offline",
            "--json",
        ])
        .env("BEAD_FAILPOINTS", "clock@always=offset(-180d)")
        .run();
    assert_eq!(old.returncode(), 0, "{}", old.stderr);
    here.bead(["update", &key, "--title", "kept working"]);
    here.bead(["create", "meanwhile"]);
    let synced = away.run(["sync", "--json"]);
    assert_eq!(synced.returncode(), 0, "{}", synced.stderr);
    here.bead(["sync"]);
    for machine in [&away, &here] {
        let shown = machine.show(&key);
        let texts: Vec<&str> = (shown["comments"].as_array().expect("its comments").iter())
            .filter_map(|comment| comment["text"].as_str())
            .collect();
        assert_eq!(texts, ["written months ago"], "{}", machine.name());
        // stamped months before the issue it comments on was made: ISO times sort as text
        let written = shown["comments"][0]["created_at"]
            .as_str()
            .expect("its time");
        let made = shown["created_at"].as_str().expect("the issue's time");
        assert!(written < made, "{written} is not before {made}");
        assert_eq!(shown["title"].as_str(), Some("kept working"));
        let info = machine.bead(["info"]).expect("bead info prints a record");
        assert_eq!(
            info["integrity"],
            Json::Array(Vec::new()),
            "{}",
            machine.name()
        );
    }
}

/// F14: a clock past year 2100 must not crash create. Set every clock reading to the year 2110
/// with `clock@always=at(4429000000000000000)`.
#[test]
fn future_clock_create_is_a_structured_error() {
    let sandbox = crate::sandbox();
    let machine = machine(&sandbox, "f14");
    let created = machine
        .cmd(["create", "future", "--json"])
        .env("BEAD_FAILPOINTS", "clock@always=at(4429000000000000000)")
        .run();
    assert!(!created.crashed(), "{}", created.stderr);
    assert_ne!(created.returncode(), 0, "{}", created.stdout);
    assert!(reported_code(&created).is_some(), "{}", created.stderr);
    let listed = machine.bead(["list", "--all", "--limit", "0"]);
    let listed = listed.expect("bead list prints a record");
    let issues = listed["issues"].as_array().expect("a list of issues");
    let titles: Vec<&str> = issues
        .iter()
        .map(|issue| issue["title"].as_str().expect("a title"))
        .collect();
    assert!(!titles.contains(&"future"), "{titles:?}");
}

/// A write that has seen another still settles it when its clock is behind. Set
/// `clock@always=offset(-48h)` for two updates by `clock-future`. Check that each timestamp is
/// more than 47 hours before the edit it follows or races, so the test proves the clock shifted
/// as well as checking the resulting registers.
#[test]
fn clock_skew_does_not_override_causality() {
    let sandbox = crate::sandbox();
    let behind = |machine: &Machine, key: &str, title: &str| {
        let update = machine.cmd(["update", key, "--title", title, "--json"]);
        update.env("BEAD_FAILPOINTS", BEHIND).run()
    };
    let past = machine(&sandbox, "clock-past");
    let key = printed_id(past.bead(["create", "base"]));
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

    let key2 = printed_id(past.bead(["create", "race"]));
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
