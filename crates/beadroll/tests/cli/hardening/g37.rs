//! G37: close, delete and `update --status closed` by the owner publish at once (`publish_soon`);
//! when that publish fails, text mode prints "recorded locally, not yet published (CODE)", but the
//! `--json` answer is the issue list alone, identical to a published one. An agent that reads
//! `--json` (the mode `bead prime` tells it to use) believes the other machines know the claim has
//! ended.

mod g37_json_says_when_the_end_of_a_claim_was_not_published {
    use beadroll_test_support::{Json, Machine, Run, Sandbox, parse_json};

    /// The class's `setUp`: machine a's tracker (prefix `gj`) with five issues, published, each
    /// claimed by a; their keys in the order `closed`, `deleted`, `updated`, `control`, `text`.
    fn setup(sandbox: &Sandbox) -> (Machine, Vec<String>) {
        let a = Machine::new(sandbox, "a");
        a.bead(["init", "--prefix", "gj"]);
        let keys: Vec<String> = ["closed", "deleted", "updated", "control", "text"]
            .into_iter()
            .map(|name| {
                let created = a.bead(["create", name]).expect("the new issue");
                created["id"].as_str().expect("an id").to_owned()
            })
            .collect();
        a.bead(["sync"]);
        for key in &keys {
            a.bead(["claim", key]);
        }
        (a, keys)
    }

    /// The one issue of a `--json` answer: `json.loads(stdout)[0]`, which must be there.
    fn first(answer: &Json) -> &Json {
        let issues = answer.as_array().expect("the answer is a list");
        issues.first().expect("the answer holds an issue")
    }

    /// The members of the issue `issue`, which must be an object.
    fn members(issue: &Json) -> &beadroll_core::json::Object {
        issue.as_object().expect("an issue is an object")
    }

    /// The runs the test makes while the remote is away, in its order: `close`, `delete` and
    /// `update --status closed` with `--json`, then a close in text mode.
    fn runs_without_the_remote(a: &Machine, keys: &[String]) -> [(&'static str, Run); 4] {
        [
            (
                "close",
                a.run(["close", &keys[0], "--reason", "done", "--json"]),
            ),
            (
                "delete",
                a.run(["delete", &keys[1], "--reason", "gone", "--json"]),
            ),
            (
                "update",
                a.run(["update", &keys[2], "--status", "closed", "--json"]),
            ),
            (
                "close (text)",
                a.run(["close", &keys[4], "--reason", "done"]),
            ),
        ]
    }

    /// G37: with `--json`, an owner's close/delete/update --status closed whose publish failed
    /// must say so.
    #[test]
    fn g37_json_close_delete_update_say_they_were_not_published() {
        let sandbox = crate::sandbox();
        let (a, keys) = setup(&sandbox);
        // the remote answers
        let control = a.run(["close", &keys[3], "--reason", "done", "--json"]);
        assert_eq!(control.returncode(), 0, "{}", control.stderr);
        let answer = parse_json(&control.stdout).unwrap();
        assert!(
            !members(first(&answer)).contains_key("unpublished"),
            "a published close is not marked"
        );
        let away = sandbox.root().join("origin-away.git");
        // the remote is unreachable from here on
        std::fs::rename(sandbox.origin(), &away).unwrap();
        let runs = runs_without_the_remote(&a, &keys);
        std::fs::rename(&away, sandbox.origin()).unwrap();
        let [close, delete, update, text] = runs;
        assert!(
            text.1.stdout.contains("not yet published"),
            "text mode is told"
        );
        for (name, proc) in [close, delete, update] {
            assert_eq!(proc.returncode(), 0, "{name}: {}", proc.stderr);
            let answer = parse_json(&proc.stdout).unwrap();
            let issues = answer
                .as_array()
                .expect("the --json answer keeps its shape: a list of issues");
            assert_eq!(issues.len(), 1, "{name}");
            let unpublished = match members(&issues[0]).get("unpublished") {
                Some(note) => note.as_str().expect("the note is text"),
                None => "",
            };
            assert!(
                unpublished.contains("not yet published"),
                "{name} --json: the publish failed, yet the answer carries no sign of it (stdout \
                 {:?}, stderr {:?})",
                proc.stdout.trim().chars().take(160).collect::<String>(),
                proc.stderr.trim()
            );
        }
    }
}
