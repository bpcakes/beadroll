//! Count bookkeeping after a successful publication must preserve a committed command's answer.

use beadroll_core::json::Value;
use beadroll_test_support::{code, parse_json};

use super::super::on_remote;
use super::{durable_inputs, id, json, published_three};

/// AG01: publication clears the pending-write flag even when saving its counts fails. Local
/// appends, init's own publication and coordinated ownership must all retain committed success.
#[test]
fn ag01_a_published_write_keeps_its_result_when_saving_counts_fails() {
    for command in ["comment", "init", "claim"] {
        let sandbox = crate::sandbox();
        let r = crate::rust_only(&sandbox, "r");
        let (meta, original, segment, published) = published_three(&r);
        let key = id(&json(&r, &["list"])["issues"][0]);
        let edited = original.replacen(
            &published,
            &format!("\"{segment}\": \"{}\"", 3 - i128::MAX),
            1,
        );
        std::fs::write(&meta, &edited).unwrap();
        let listed = r.run(["list", "--json"]);
        assert_eq!(listed.returncode(), 0, "{}", listed.stderr);
        assert!(
            listed
                .stdout
                .contains(&format!("\"unpublished\": {}", i128::MAX))
        );
        let before = durable_inputs(&r.store().join("events"));
        let import = sandbox.root().join("extra.jsonl");
        std::fs::write(&import, "{\"id\":\"ex-imported\",\"title\":\"imported\"}\n").unwrap();
        let (mut args, fault) = match command {
            // init first saves its chosen remote successfully; its publication saves next.
            "init" => (
                vec![
                    "init",
                    "--remote",
                    "origin",
                    "--import",
                    import.to_str().unwrap(),
                ],
                "atomic-write:meta.json@nth(2)=errno(ENOSPC)",
            ),
            "claim" => (vec!["claim", &key], "atomic-write:meta.json=errno(ENOSPC)"),
            _ => (
                vec!["comments", "add", &key, "once"],
                "atomic-write:meta.json=errno(ENOSPC)",
            ),
        };
        args.push("--json");
        let written = r.cmd(args).env("BEAD_FAILPOINTS", fault).run();
        assert_eq!(written.returncode(), 0, "{command}: {}", written.stderr);
        let result = parse_json(&written.stdout).unwrap();
        assert_result(command, &result, &key);
        let warning = parse_json(&written.stderr).unwrap();
        assert_eq!(warning["warning"]["code"].as_str(), Some("IO"));
        assert!(
            warning["warning"]["message"]
                .as_str()
                .unwrap()
                .contains("write recorded locally;")
        );
        // The remote has the new event, but the old counts remain: the save fault fired after
        // successful publication. The command did not merely fall back to an offline write.
        let after = durable_inputs(&r.store().join("events"));
        assert_ne!(after, before, "{command}");
        for (path, bytes) in &after {
            let relative = path.strip_prefix(r.store()).unwrap().to_str().unwrap();
            let object = format!("refs/beads/journal:{relative}");
            assert_eq!(
                on_remote(sandbox.origin(), &["show", &object]).as_bytes(),
                bytes
            );
        }
        let saved = parse_json(&std::fs::read_to_string(&meta).unwrap()).unwrap();
        assert_eq!(
            saved["published"],
            parse_json(&edited).unwrap()["published"]
        );
        let refused = r.run(["list", "--json"]);
        assert_eq!(code(&refused), "IO");
        assert_eq!(durable_inputs(&r.store().join("events")), after);
        std::fs::write(&meta, &original).unwrap();
        if command == "comment" {
            let shown = json(&r, &["show", &key]);
            assert_eq!(shown[0]["comments"].as_array().unwrap().len(), 1);
        }
    }
}

fn assert_result(command: &str, result: &Value, key: &str) {
    match command {
        "init" => assert_eq!(result["imported"].as_i64(), Some(1)),
        "claim" => assert_eq!(result[0]["id"].as_str(), Some(key)),
        _ => assert_eq!(result["text"].as_str(), Some("once")),
    }
}
