//! G32 (`docs/FINDINGS.md`): integer fields stay readable by Beads. The test's replay and
//! `read_attempt` half is beadroll-store's (`tests/store/hardening.rs`); this is its command-line
//! half.

mod g32_integer_fields_stay_beads_readable {
    use beadroll_core::json::{Format, Object, write};
    use beadroll_test_support::{Json, Machine, Run, code, json};

    /// `_g32_common`: a Beads record's required fields.
    fn common() -> Json {
        json!({
            "status": "open", "priority": 2, "issue_type": "task",
            "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
            "created_by": "seed"
        })
    }

    /// `assert_refused`: the write was refused with VALIDATION, and bead did not crash.
    fn assert_refused(run: &Run, what: &str) {
        assert!(!run.crashed(), "{what} crashed: {}", run.stderr);
        assert_ne!(
            run.returncode(),
            0,
            "{what} was accepted: {}",
            run.stdout.trim()
        );
        assert_eq!(code(run), "VALIDATION", "{what}");
    }

    /// G32: bead accepted a JSON boolean for priority/estimated_minutes (bool is an int) and an
    /// estimated_minutes of any sign or size, then exported records br refuses (`expected i32`,
    /// `cannot be negative`). New writes must refuse them: imports, `create --estimate`, and
    /// `update --estimate` with or without `--claim`, refused whole, so that no claim is taken;
    /// the largest value Beads reads is still accepted. (That histories already holding such values
    /// still replay is the store half.)
    #[test]
    fn writes_refuse_booleans_and_estimates_beads_cannot_read() {
        let sandbox = crate::sandbox();
        let m = Machine::new(&sandbox, "g32");
        m.bead(["init", "--prefix", "ex"]);
        let cases = [
            ("prio-true", json!({"priority": true})),
            ("prio-false", json!({"priority": false})),
            ("est-bool", json!({"estimated_minutes": false})),
            ("est-neg", json!({"estimated_minutes": (-7)})),
            ("est-big", json!({"estimated_minutes": (2_147_483_648_i64)})),
        ];
        for (name, fields) in cases {
            // {"id": ..., "title": ..., **_g32_common(), **fields}
            let mut record = Object::new();
            record.insert("id".into(), Json::from(format!("ex-{name}")));
            record.insert("title".into(), Json::from(name));
            for part in [common(), fields] {
                let Json::Object(part) = part else {
                    panic!("members, not {part:?}");
                };
                record.extend(*part);
            }
            let path = sandbox.root().join(format!("{name}.jsonl"));
            let line = write(&Json::from(record), Format::ASCII).unwrap() + "\n";
            std::fs::write(&path, line).unwrap();
            let path = path.display().to_string();
            assert_refused(
                &m.run(["import", &path, "--json"]),
                &format!("import of {name}"),
            );
        }
        for value in ["-7", "2147483648"] {
            let estimate = format!("--estimate={value}");
            assert_refused(
                &m.run(["create", "e", &estimate, "--json"]),
                &format!("create {estimate}"),
            );
        }
        let created = m.bead(["create", "ok", "--estimate=0"]).unwrap();
        let key = created["id"].as_str().unwrap().to_owned();
        m.bead(["sync"]);
        for args in [
            vec!["update", key.as_str(), "--estimate=-7"],
            vec!["update", key.as_str(), "--claim", "--estimate=-7"],
        ] {
            let mut all = args.clone();
            all.push("--json");
            assert_refused(&m.run(all), &args.join(" "));
        }
        let issue = m.show(&key);
        let estimate = issue.get("estimated_minutes").and_then(Json::as_i64);
        assert_eq!(
            (estimate, issue["status"].as_str()),
            (Some(0), Some("open")),
            "refused whole, no claim"
        );
        // the largest value Beads reads is still accepted
        m.bead(["create", "edge", "--estimate=2147483647"]);
        assert!(!m.export().contains("\"priority\":true"));
    }
}
