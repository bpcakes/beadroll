//! `test_protocol`'s test of the core: ancestry across ownership files and journals.

mod protocol3 {
    use beadroll_core::journal::Journal;
    use beadroll_core::json::{Format, Value, write};
    use beadroll_core::replay::replay;
    use beadroll_test_support::events::{Event, id_at};
    use beadroll_test_support::json;

    /// A splitmix64 stream: the property holds for any order, so any seeded stream will do.
    struct Stream(u64);

    impl Stream {
        fn below(&mut self, n: usize) -> usize {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            usize::try_from(z % u64::try_from(n).unwrap()).unwrap()
        }
    }

    /// 2,000 events on six machines, one chain through all of them, claims and releases in
    /// attempt files and the rest in journals: whichever order the machines are read in, nothing is
    /// flagged or pending, every event saw exactly the events before it, and the issue reads the same.
    #[test]
    fn mixed_ownership_file_and_journal_ancestry_across_six_machines() {
        let mut events: Vec<Value> = Vec::new();
        let (mut previous, mut active, mut uid) = (None::<String>, String::new(), String::new());
        for index in 0..2000u64 {
            let eid = id_at(1_770_000_000_000 + index, 0);
            let replica = format!("machine{}", index % 6);
            let (op, data, issue) = match index {
                0 => ("init", json!({"prefix": "ex"}), eid.clone()),
                1 => {
                    uid.clone_from(&eid);
                    (
                        "create",
                        json!({"key": "ex-mixed", "fields": {"title": "mixed"}}),
                        eid.clone(),
                    )
                }
                _ if index % 10 == 2 => {
                    active.clone_from(&eid);
                    (
                        "claim",
                        json!({"mode": "online", "generation": (i64::try_from(index / 10 + 1).unwrap())}),
                        uid.clone(),
                    )
                }
                _ if index % 10 == 7 => {
                    ("release", json!({"claim": (active.as_str())}), uid.clone())
                }
                _ => (
                    "comment",
                    json!({"text": "ordinary journal event"}),
                    uid.clone(),
                ),
            };
            let deps: Vec<&str> = previous.iter().map(String::as_str).collect();
            let event = Event::new(&eid, op)
                .replica(&replica)
                .actor(json!({"agent": "audit", "thread": "one-owner"}))
                .deps(&deps)
                .issue(&issue)
                .data(data);
            events.push(event.json());
            previous = Some(eid);
        }
        let mut stream = Stream(3106);
        let mut expected = None;
        for _ in 0..6 {
            let mut replicas: Vec<String> = (0..6).map(|i| format!("machine{i}")).collect();
            for i in (1..replicas.len()).rev() {
                replicas.swap(i, stream.below(i + 1));
            }
            let mut journal = Journal::new();
            for replica in &replicas {
                for event in events
                    .iter()
                    .filter(|e| e["replica"] == Value::from(replica.as_str()))
                {
                    let op = event["op"].as_str().unwrap();
                    journal.add(
                        event.clone(),
                        replica,
                        "",
                        !matches!(op, "claim" | "release"),
                    );
                }
            }
            let state = replay(journal).unwrap();
            assert!(state.integrity().is_empty(), "{:?}", state.integrity());
            assert!(state.pending().is_empty());
            let ids: Vec<_> = events
                .iter()
                .map(|e| beadroll_core::id::EventId::parse(e["id"].as_str().unwrap()).unwrap())
                .collect();
            for _ in 0..1000 {
                let first = stream.below(ids.len());
                let mut second = stream.below(ids.len() - 1);
                if second >= first {
                    second += 1;
                }
                assert_eq!(
                    state.saw(ids[first], ids[second]),
                    first > second,
                    "{first} saw {second}"
                );
            }
            let issue = state.issue_named(&uid).unwrap();
            let snapshot = write(&Value::from(state.detail(issue)), Format::CANONICAL).unwrap();
            match &expected {
                None => expected = Some(snapshot),
                Some(expected) => assert_eq!(&snapshot, expected),
            }
        }
    }
}
