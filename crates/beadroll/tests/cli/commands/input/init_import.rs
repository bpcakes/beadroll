//! `init --import`: the prefix a tracker started from an export takes, as Beads keyed its records
//! (the ids, `.beads/config.yaml`, the repository directory's name), the export read once, from a
//! file or standard input, and nothing written for an export it refuses.

use beadroll_test_support::{Json, code, json, json_lines, parse_json};

use super::record;

/// `init --import` without `--prefix` starts the tracker under the prefix its records share, so
/// that what is made after joins them, slug-keyed ids among them as real exports hold them;
/// records that share none are `VALIDATION` asking for `--prefix`, and `--prefix` given wins.
#[test]
fn init_import_takes_the_prefix_its_records_share() {
    // each its own sandbox: a second `init` beside a published tracker joins it, prefix and all
    let start = |name: &str, records: &[Json], given: &[&str]| {
        let sandbox = crate::sandbox();
        let file = sandbox.root().join(format!("{name}.jsonl"));
        std::fs::write(&file, json_lines(records)).unwrap();
        let machine = beadroll_test_support::Machine::new(&sandbox, name);
        let mut args = vec!["init"];
        args.extend_from_slice(given);
        args.extend(["--import", file.to_str().unwrap(), "--json"]);
        let run = machine.run(args);
        (sandbox, machine, run)
    };
    let shared = [
        record("bd-1", "one", json!({})),
        record("bd-2", "two", json!({})),
    ];
    let (_kept, a, run) = start("a", &shared, &[]);
    assert_eq!(run.returncode(), 0, "{}", run.stderr);
    assert_eq!(a.bead(["info"]).unwrap()["prefix"], json!("bd"));
    let made = a.bead(["create", "after"]).unwrap();
    assert!(made["id"].as_str().unwrap().starts_with("bd-"), "{made:?}");
    let mixed = [
        record("bd-1", "one", json!({})),
        record("xy-2", "two", json!({})),
    ];
    let (_kept, _b, refused) = start("b", &mixed, &[]);
    assert_eq!(code(&refused), "VALIDATION", "{}", refused.stderr);
    assert!(refused.stderr.contains("--prefix"), "{}", refused.stderr);
    let (_kept, c, run) = start("c", &shared, &["--prefix", "own"]);
    assert_eq!(run.returncode(), 0, "{}", run.stderr);
    assert_eq!(c.bead(["info"]).unwrap()["prefix"], json!("own"));
    // ids as real Beads exports hold them: `<prefix>-<hash>`, and `<prefix>-<slug>-<hash>` beside
    // them, the prefix itself holding a `-` or not; the prefix is what every id shares
    let shapes = [
        (
            "d",
            [
                "syrup-rail-0u9",
                "syrup-rail-simplification-audit-x1",
                "syrup-rail-0ym",
            ],
            "syrup-rail",
        ),
        (
            "e",
            [
                "br-account-research-fap",
                "br-account-research-fap.1",
                "br-onboarding-style-q2",
            ],
            "br",
        ),
    ];
    for (name, ids, prefix) in shapes {
        let records: Vec<Json> = ids.iter().map(|id| record(id, id, json!({}))).collect();
        let (_kept, machine, run) = start(name, &records, &[]);
        assert_eq!(run.returncode(), 0, "{name}: {}", run.stderr);
        assert_eq!(
            machine.bead(["info"]).unwrap()["prefix"],
            json!(prefix),
            "{name}"
        );
    }
}

/// One case of an export kept where Beads keeps it: the directory `.beads` is in, the records' ids,
/// what `config.yaml` holds, and the prefix the tracker starts under, or the code it is refused with
/// and what its message says.
type Keyed = (
    &'static str,
    &'static [&'static str],
    Option<&'static str>,
    Result<&'static str, (&'static str, &'static str)>,
);

/// `init --import` of an export where Beads keeps it, `DIR/.beads/issues.jsonl`, takes the prefix
/// Beads keyed it under: the `issue_prefix` its `config.yaml` names, which a lone slug-keyed id
/// (`br-onboarding-style-q2`) cannot show, else the directory's name, Beads' default, which a slug
/// may follow (`proj-fix-login-x1`). Records outside the configured prefix are `VALIDATION`.
#[test]
fn init_import_takes_the_prefix_beads_keyed_the_records_under() {
    let cases: [Keyed; 9] = [
        (
            "onesales",
            &["br-onboarding-style-q2"],
            Some("issue_prefix: br\n"),
            Ok("br"),
        ),
        (
            "proj",
            &["proj-fix-login-x1", "proj-fix-logout-x2"],
            Some("# issue_prefix: x\n"),
            Ok("proj"),
        ),
        (
            "mixed",
            &["zz-1", "br-2"],
            Some("issue_prefix: \"br\"\n"),
            Err(("VALIDATION", "not all keyed under br")),
        ),
        // a byte order mark, and a space before the colon
        (
            "marked",
            &["br-x-q2"],
            Some("\u{feff}issue_prefix: br\n"),
            Ok("br"),
        ),
        (
            "spaced",
            &["br-x-q2"],
            Some("issue-prefix : br\n"),
            Ok("br"),
        ),
        // YAML that is not a plain or quoted scalar is reported, never taken for the prefix
        (
            "block",
            &["br-x-q2"],
            Some("issue_prefix: |\n  br\n"),
            Err(("VALIDATION", "YAML")),
        ),
        (
            "tagged",
            &["br-x-q2"],
            Some("issue_prefix: !!str br\n"),
            Err(("VALIDATION", "YAML")),
        ),
        // bd's spelling, and a comment after a tab
        (
            "bd",
            &["br-onboarding-style-q2"],
            Some("issue-prefix: \"br\"\n"),
            Ok("br"),
        ),
        (
            "tabbed",
            &["br-x-q2"],
            Some("issue_prefix: br\t# the prefix\n"),
            Ok("br"),
        ),
    ];
    for (dir, ids, config, expected) in cases {
        let sandbox = crate::sandbox();
        let beads = sandbox.root().join(dir).join(".beads");
        std::fs::create_dir_all(&beads).unwrap();
        let records: Vec<Json> = ids.iter().map(|id| record(id, id, json!({}))).collect();
        std::fs::write(beads.join("issues.jsonl"), json_lines(&records)).unwrap();
        if let Some(config) = config {
            std::fs::write(beads.join("config.yaml"), config).unwrap();
        }
        let machine = beadroll_test_support::Machine::new(&sandbox, "a");
        let file = beads.join("issues.jsonl");
        let run = machine.run(["init", "--import", file.to_str().unwrap(), "--json"]);
        match expected {
            Ok(prefix) => {
                assert_eq!(run.returncode(), 0, "{dir}: {}", run.stderr);
                assert_eq!(
                    machine.bead(["info"]).unwrap()["prefix"],
                    json!(prefix),
                    "{dir}"
                );
            }
            Err((failure, says)) => {
                assert_eq!(code(&run), failure, "{dir}: {}", run.stderr);
                assert!(run.stderr.contains(says), "{dir}: {}", run.stderr);
            }
        }
    }
}

/// `init --import -` reads the export from standard input once, for its prefix and its import alike:
/// the prefix's read took every byte, and the import found nothing and answered `imported: 0`. Its
/// dry run counts what it would import, and nothing beside standard input is read: a
/// `config.yaml` directory where the command runs is no Beads configuration of it.
#[test]
fn init_imports_standard_input_once_for_its_prefix_and_its_records() {
    let sandbox = crate::sandbox();
    let records = json_lines(&[
        record("bd-1", "one", json!({})),
        record("bd-2", "two", json!({})),
    ]);
    let machine = beadroll_test_support::Machine::new(&sandbox, "a");
    std::fs::create_dir(machine.checkout().join("config.yaml")).unwrap();
    let planned = machine
        .cmd(["init", "--import", "-", "--dry-run", "--json"])
        .stdin(records.clone())
        .run();
    assert_eq!(planned.returncode(), 0, "{}", planned.stderr);
    let planned = parse_json(&planned.stdout).unwrap();
    assert_eq!(planned["importable"], json!(true), "{planned:?}");
    let started = machine
        .cmd(["init", "--import", "-", "--json"])
        .stdin(records)
        .run();
    assert_eq!(started.returncode(), 0, "{}", started.stderr);
    let started = parse_json(&started.stdout).unwrap();
    assert_eq!(
        (started["prefix"].clone(), started["imported"].clone()),
        (json!("bd"), json!(2))
    );
    let listed = machine.bead(["list", "--all", "--limit", "0"]).unwrap();
    assert_eq!(listed["total"], json!(2), "{listed:?}");
}

/// An export `init --import` refuses leaves nothing written: it is read before a remote or a
/// workspace is chosen, where a refused one left a `.beadroll.json` with a new id behind, or a
/// published tracker already joined.
#[test]
fn a_refused_init_import_writes_nothing() {
    let sandbox = crate::sandbox();
    let machine = beadroll_test_support::Machine::new(&sandbox, "a");
    let bad = sandbox.root().join("bad.jsonl");
    std::fs::write(&bad, b"\xff\n").unwrap();
    let run = machine.run([
        "init",
        "--tracked",
        "--import",
        bad.to_str().unwrap(),
        "--json",
    ]);
    assert_eq!(code(&run), "VALIDATION", "{}", run.stderr);
    assert!(
        !machine.checkout().join(".beadroll.json").exists(),
        "a tracked file was written"
    );
    assert!(!machine.home().exists(), "a store was started");
}

/// `init --import --dry-run` reads the prefix as `init --import` would: records under several
/// prefixes are a problem it lists, where it said the import would go through, and `--prefix`,
/// which settles it, is taken.
#[test]
fn an_init_dry_run_reads_the_prefix_as_the_init_would() {
    let sandbox = crate::sandbox();
    let mixed = sandbox.root().join("mixed.jsonl");
    std::fs::write(
        &mixed,
        json_lines(&[
            record("ap-1", "a", json!({})),
            record("zz-1", "b", json!({})),
        ]),
    )
    .unwrap();
    let machine = beadroll_test_support::Machine::new(&sandbox, "plan");
    let planned = machine
        .bead(["init", "--import", mixed.to_str().unwrap(), "--dry-run"])
        .unwrap();
    assert_eq!(planned["importable"], json!(false), "{planned:?}");
    assert!(
        planned["problems"][0]
            .as_str()
            .unwrap()
            .contains("--prefix"),
        "{planned:?}"
    );
    let given = [
        "init",
        "--import",
        mixed.to_str().unwrap(),
        "--dry-run",
        "--prefix",
        "pp",
    ];
    let planned = machine.bead(given).unwrap();
    assert_eq!(planned["importable"], json!(true), "{planned:?}");
}
