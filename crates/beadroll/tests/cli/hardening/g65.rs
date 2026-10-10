//! G65: an owner's close that a takeover has already superseded does not answer as if it closed.

mod g65_superseded_owner_close_says_so {
    use beadroll_test_support::{Machine, parse_json};

    /// G65: an owner's close (or delete) published right after another machine's takeover is
    /// ignored everywhere and listed under stale, as REVIEWING section 2 decides; but the command
    /// still answers '✓ Closed' / status closed with exit 0, although its own publish has just
    /// fetched the takeover and this machine now shows the issue in_progress under the new owner.
    #[test]
    fn close_superseded_by_a_takeover_does_not_answer_closed() {
        let sandbox = crate::sandbox();
        let a = Machine::new(&sandbox, "a");
        let b = Machine::new(&sandbox, "b");
        a.bead(["init", "--prefix", "so"]);
        let created = a.bead(["create", "x"]).expect("the created issue");
        let key = created["id"].as_str().expect("an id").to_owned();
        a.bead(["sync"]);
        b.bead(["sync"]);
        a.bead(["claim", &key]);
        b.bead(["sync"]);
        b.bead(["takeover", &key, "--reason", "a looked dead"]);
        let closed = a.run(["close", &key, "--reason", "done", "--json"]);
        let shown = a.show(&key);
        let stale: Vec<String> = shown.get("stale").map_or_else(Vec::new, |stale| {
            let entries = stale.as_array().expect("stale is a list");
            let op = |entry: &beadroll_test_support::Json| {
                entry["op"].as_str().expect("an op").to_owned()
            };
            entries.iter().map(op).collect()
        });
        let status = shown["status"].as_str().expect("a status");
        assert_eq!((status, stale), ("in_progress", vec!["close".to_owned()]));
        let answered = (closed.returncode() == 0).then(|| {
            let result = parse_json(&closed.stdout).expect("close printed JSON");
            let status = result[0]["status"]
                .as_str()
                .expect("the closed issue's status");
            status.to_owned()
        });
        assert_ne!(
            (closed.returncode(), answered.as_deref()),
            (0, Some("closed")),
            "the close answered closed with exit 0, yet it was superseded and ignored: {}",
            closed.stdout
        );
    }
}
