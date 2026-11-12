//! Publication counts must describe the locked round, before another process writes offline.

use beadroll_test_support::parse_json;

use crate::hardening::gate::{Gate, beside};

#[test]
fn writes_after_the_sync_round_remain_unpublished() {
    for scenario in ["append", "rotate", "claim", "automatic", "unchanged"] {
        let sandbox = crate::sandbox();
        let a = crate::rust_only(&sandbox, "a");
        let b = crate::rust_only(&sandbox, "b");
        a.bead(["init", "--prefix", "ex"]);
        let created = a.bead(["create", "issue"]).unwrap();
        let key = created["id"].as_str().unwrap();
        if scenario != "unchanged" {
            a.bead(["comments", "add", key, "queued", "--offline"]);
        }
        let window = Gate::new(&sandbox, "after-round");
        let faults = format!("sync.after-round={}", window.action());
        let (published, met) = beside(
            || {
                let args = if scenario == "automatic" {
                    vec!["comments", "add", key, "online", "--json"]
                } else {
                    vec!["sync", "--json"]
                };
                a.cmd(args).env("BEAD_FAILPOINTS", &faults).run()
            },
            |finished| {
                window.meet(finished, || {
                    let args = if scenario == "claim" {
                        vec!["claim", key, "--offline", "--json"]
                    } else {
                        vec!["comments", "add", key, "kept local", "--offline", "--json"]
                    };
                    let command = a.cmd(args);
                    let written = if scenario == "rotate" {
                        command.env("BEAD_ROTATE_BYTES", "1").run()
                    } else {
                        command.run()
                    };
                    assert_eq!(written.returncode(), 0, "{}", written.stderr);
                })
            },
        );
        assert!(met.is_some(), "{scenario}: the unlocked window was reached");
        assert_eq!(published.returncode(), 0, "{}", published.stderr);

        // Another machine observes the actual published tree, independently of a's metadata.
        b.bead(["sync"]);
        let remote = b.show(key);
        assert!(remote.get("claim").is_none(), "{scenario}: {remote:?}");
        assert!(remote.get("provisional_claims").is_none());
        assert!(
            remote["comments"]
                .as_array()
                .unwrap()
                .iter()
                .all(|comment| { comment["text"].as_str() != Some("kept local") })
        );
        let answer = parse_json(&published.stdout).unwrap();
        assert_eq!(answer["unpublished"].as_i64(), Some(1), "{scenario}");
        assert_eq!(
            a.bead(["info"]).unwrap()["unpublished"].as_i64(),
            Some(1),
            "{scenario}: a fresh process reads the saved counts"
        );
        a.bead(["sync"]);
        assert!(a.bead(["info"]).unwrap().get("unpublished").is_none());
        b.bead(["sync"]);
        let remote = b.show(key);
        if scenario == "claim" {
            assert_eq!(remote["provisional_claims"].as_array().unwrap().len(), 1);
        } else {
            assert!(
                remote["comments"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|comment| { comment["text"].as_str() == Some("kept local") })
            );
        }
    }
}
