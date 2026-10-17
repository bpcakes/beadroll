//! `State::dependents_of`, the issues that depend on each of several issues found in one pass over
//! every issue's edges (roll-lqjj.20), against the reading every detail did on its own before:
//! every issue in replay order, its edges sorted, those that name the one issue's uid. A detail
//! made with it is `State::detail` of the issue alone, member for member and in the same order, for
//! replay order that differs from uid and key order, several types between one pair, an edge added
//! twice, removed edges, dependencies on keys and uids that are no issue's, closed and deleted
//! sources, a key two machines minted, issues asked about twice or not at all, and random graphs.

use beadroll_core::event::Field;
use beadroll_core::json::{Format, write};
use beadroll_core::state::{Issue, State};
use beadroll_test_support::events::{Event, id, state};
use beadroll_test_support::{Json, json};

/// A history built event by event, each on machine `a` after the one before unless it says
/// otherwise: the events and the number of the last.
struct History {
    events: Vec<Json>,
    last: Option<String>,
}

impl History {
    /// An `init` of prefix `dd`, numbered 1.
    fn new() -> History {
        let init = Event::new(&id(1), "init").data(json!({"prefix": "dd", "name": "dependents"}));
        History {
            events: vec![init.json()],
            last: Some(id(1)),
        }
    }

    /// Adds `event`, numbered `n`, on machine `replica`, after the last event; returns its id.
    fn add(&mut self, n: u64, replica: &str, op: &str, issue: Option<&str>, data: Json) -> String {
        let eid = id(n);
        let deps: Vec<&str> = self.last.as_deref().into_iter().collect();
        let mut event = Event::new(&eid, op).replica(replica).deps(&deps).data(data);
        if let Some(issue) = issue {
            event = event.issue(issue);
        }
        self.events.push(event.json());
        self.last = Some(eid.clone());
        eid
    }

    /// Creates the issue `key`, its event numbered `n`; returns its uid.
    fn create(&mut self, n: u64, key: &str) -> String {
        self.add(
            n,
            "a",
            "create",
            None,
            json!({"key": key, "fields": {"title": key}}),
        )
    }

    /// Adds a dependency of `kind` from `from` to `to`, numbered `n`.
    fn link(&mut self, n: u64, from: &str, to: &str, kind: &str) {
        self.add(
            n,
            "a",
            "dep_add",
            Some(from),
            json!({"to": to, "type": kind}),
        );
    }

    /// Removes the dependency of `kind` from `from` to `to`, numbered `n`.
    fn unlink(&mut self, n: u64, from: &str, to: &str, kind: &str) {
        self.add(
            n,
            "a",
            "dep_remove",
            Some(from),
            json!({"to": to, "type": kind}),
        );
    }

    fn state(&self) -> State {
        state(&self.events)
    }
}

/// A value as written, members in their order: `Json` equality does not see the order.
fn text(value: &Json) -> String {
    write(value, Format::COMPACT).unwrap()
}

/// What a detail listed as `issue`'s dependents before roll-lqjj.20: every issue in replay order,
/// its edges sorted, those that name `issue`'s uid.
fn reference(st: &State, issue: &Issue) -> Json {
    let uid = issue.uid().to_string();
    let mut found = Vec::new();
    for other in st.issues() {
        let mut edges: Vec<(String, String)> = other
            .deps()
            .keys()
            .map(|edge| (edge.to().to_string(), edge.kind().to_owned()))
            .collect();
        edges.sort();
        for (_, kind) in edges.into_iter().filter(|(to, _)| *to == uid) {
            let field = |name| other.field(name).cloned().unwrap_or(Json::Null);
            found.push(json!({
                "id": (other.key()),
                "title": (field(Field::Title)),
                "status": (field(Field::Status)),
                "dependency_type": (kind.as_str())
            }));
        }
    }
    Json::Array(found)
}

/// The details of `asked`, in its order, made with their dependents found once for all of them,
/// and with readiness too (`State::details_of`), each against the issue's detail alone and its
/// dependents against the reference; each one's dependents as `key type` pairs.
fn agree(st: &State, asked: &[&Issue]) -> Vec<Vec<String>> {
    let dependents = st.dependents_of(asked.iter().copied());
    let details = st.details_of(asked.iter().copied());
    (asked.iter())
        .map(|issue| {
            let together = Json::from(st.detail_with(issue, &dependents));
            let alone = Json::from(st.detail(issue));
            assert_eq!(text(&together), text(&alone), "{}", issue.key());
            let shared = Json::from(details.of(issue));
            assert_eq!(text(&shared), text(&alone), "{}", issue.key());
            let listed = &together["dependents"];
            assert_eq!(text(listed), text(&reference(st, issue)), "{}", issue.key());
            (listed.as_array().unwrap().iter())
                .map(|dep| {
                    let part = |name: &str| dep[name].as_str().unwrap_or("null").to_owned();
                    format!("{} {}", part("id"), part("dependency_type"))
                })
                .collect()
        })
        .collect()
}

/// Every issue of `st`, in replay order.
fn every(st: &State) -> Vec<&Issue> {
    st.issues().collect()
}

/// The issue whose key is `key`.
fn named<'s>(st: &'s State, key: &str) -> &'s Issue {
    st.resolve(key).unwrap()
}

#[test]
fn dependents_keep_replay_order_where_uids_and_keys_sort_otherwise() {
    let mut h = History::new();
    let target = h.create(2, "dd-target");
    // Each create names the one before as its cause, so replay applies them in the order made,
    // whatever their ids: uids made later sort first, and keys sort in neither order.
    let late = h.create(90, "dd-m");
    let early = h.create(40, "dd-z");
    let middle = h.create(60, "dd-a");
    for (n, from) in [(91, &middle), (92, &late), (93, &early)] {
        h.link(n, from, &target, "blocks");
    }
    let st = h.state();
    let order: Vec<&str> = st.issues().map(Issue::key).collect();
    assert_eq!(order, ["dd-target", "dd-m", "dd-z", "dd-a"], "replay order");
    let shown = agree(&st, &[named(&st, "dd-target")]);
    assert_eq!(shown, [["dd-m blocks", "dd-z blocks", "dd-a blocks"]]);
    assert!(late > early && middle > early, "uids sort otherwise");
}

#[test]
fn several_types_between_one_pair_are_each_listed_once_in_sorted_order() {
    let mut h = History::new();
    let target = h.create(2, "dd-target");
    let source = h.create(3, "dd-source");
    let other = h.create(4, "dd-other");
    // added in an order that is not sorted, one of them twice, so that its tag set holds two
    // additions; and an edge to another issue between them, which sorts among them by its target
    h.link(5, &source, &target, "related");
    h.link(6, &source, &other, "blocks");
    h.link(7, &source, &target, "blocks");
    h.link(8, &source, &target, "discovered-from");
    h.link(9, &source, &target, "parent-child");
    h.link(10, &source, &target, "blocks");
    h.link(11, &other, &target, "related");
    let st = h.state();
    let shown = agree(&st, &[named(&st, "dd-target"), named(&st, "dd-other")]);
    assert_eq!(
        shown,
        [
            vec![
                "dd-source blocks",
                "dd-source discovered-from",
                "dd-source parent-child",
                "dd-source related",
                "dd-other related",
            ],
            vec!["dd-source blocks"],
        ]
    );
}

#[test]
fn removed_edges_are_gone_and_an_edge_added_again_is_back() {
    let mut h = History::new();
    let target = h.create(2, "dd-target");
    let source = h.create(3, "dd-source");
    h.link(4, &source, &target, "blocks");
    h.link(5, &source, &target, "related");
    h.unlink(6, &source, &target, "blocks");
    let st = h.state();
    assert_eq!(
        agree(&st, &[named(&st, "dd-target")]),
        [["dd-source related"]]
    );
    h.unlink(7, &source, &target, "related");
    let st = h.state();
    assert!(agree(&st, &every(&st)).iter().all(Vec::is_empty));
    h.link(8, &source, &target, "blocks");
    let st = h.state();
    assert_eq!(
        agree(&st, &[named(&st, "dd-target")]),
        [["dd-source blocks"]]
    );
}

#[test]
fn closed_and_deleted_sources_are_listed_with_their_status() {
    let mut h = History::new();
    let target = h.create(2, "dd-target");
    let closed = h.create(3, "dd-closed");
    let deleted = h.create(4, "dd-deleted");
    let open = h.create(5, "dd-open");
    for (n, from) in [(6, &closed), (7, &deleted), (8, &open)] {
        h.link(n, from, &target, "blocks");
    }
    h.add(9, "a", "close", Some(&closed), json!({}));
    h.add(10, "a", "delete", Some(&deleted), json!({}));
    let st = h.state();
    let dependents = st.dependents_of([named(&st, "dd-target")]);
    let detail = Json::from(st.detail_with(named(&st, "dd-target"), &dependents));
    let statuses: Vec<&str> = (detail["dependents"].as_array().unwrap().iter())
        .map(|dep| dep["status"].as_str().unwrap())
        .collect();
    assert_eq!(statuses, ["closed", "tombstone", "open"]);
    agree(&st, &every(&st));
}

#[test]
fn dependencies_on_what_is_no_issue_list_nobody_and_an_import_binds_when_its_key_comes() {
    let mut h = History::new();
    let waiting = h.add(
        2,
        "a",
        "create",
        None,
        json!({"key": "dd-waiting", "fields": {"title": "waits"}, "imported": true,
               "dangling": [{"depends_on_id": "dd-later", "type": "related"},
                            {"depends_on_id": "dd-never", "type": "blocks"}]}),
    );
    // an edge to a uid that names no issue
    let stray = id(1_000);
    h.link(3, &waiting, &stray, "blocks");
    let st = h.state();
    let shown = Json::from(st.detail(named(&st, "dd-waiting")));
    assert_eq!(
        shown["unresolved_dependencies"].as_array().unwrap().len(),
        2
    );
    assert_eq!(shown["dependencies"][0]["status"].as_str(), Some("unknown"));
    assert!(agree(&st, &every(&st)).iter().all(Vec::is_empty));
    h.create(4, "dd-later");
    let st = h.state();
    assert_eq!(
        agree(&st, &[named(&st, "dd-later"), named(&st, "dd-waiting")]),
        [vec!["dd-waiting related"], vec![]]
    );
    let shown = Json::from(st.detail(named(&st, "dd-waiting")));
    assert_eq!(
        shown["unresolved_dependencies"].as_array().unwrap().len(),
        1
    );
}

#[test]
fn a_key_two_machines_minted_keeps_each_copys_dependents_apart() {
    let mut h = History::new();
    let base = h.last.clone().unwrap();
    // two machines mint dd-twin without seeing each other; the copy that sorts second is renamed
    let mut created = Vec::new();
    for (n, replica) in [(2, "a"), (3, "b")] {
        let eid = id(n);
        let event = Event::new(&eid, "create")
            .replica(replica)
            .deps(&[&base])
            .data(json!({"key": "dd-twin", "fields": {"title": (replica)}}));
        h.events.push(event.json());
        created.push(eid);
    }
    // the source has seen both, so that its edges may name either
    let source = id(4);
    let event = Event::new(&source, "create")
        .deps(&[&created[0], &created[1]])
        .data(json!({"key": "dd-source", "fields": {"title": "source"}}));
    h.events.push(event.json());
    h.last = Some(source.clone());
    h.link(5, &source, &created[0], "blocks");
    h.link(6, &source, &created[1], "related");
    let st = h.state();
    assert_eq!(st.renames().len(), 1, "{:?}", st.renames());
    let renamed = st.renames()[0].to.clone();
    let first = named(&st, "dd-twin");
    let second = named(&st, &renamed);
    assert_ne!(first.uid(), second.uid());
    assert_eq!(
        agree(&st, &[first, second]),
        [["dd-source blocks"], ["dd-source related"]]
    );
}

#[test]
fn issues_asked_twice_or_not_at_all_get_the_detail_they_would_alone() {
    let mut h = History::new();
    let target = h.create(2, "dd-target");
    let other = h.create(3, "dd-other");
    let source = h.create(4, "dd-source");
    h.link(5, &source, &target, "blocks");
    h.link(6, &source, &other, "related");
    h.link(7, &other, &target, "parent-child");
    let st = h.state();
    let (target, other, source) = (
        named(&st, "dd-target"),
        named(&st, "dd-other"),
        named(&st, "dd-source"),
    );
    let shown = agree(&st, &[target, other, target, source, other]);
    assert_eq!(
        shown,
        [
            vec!["dd-other parent-child", "dd-source blocks"],
            vec!["dd-source related"],
            vec!["dd-other parent-child", "dd-source blocks"],
            vec![],
            vec!["dd-source related"],
        ]
    );
    // an issue the lookup was not made for is found on its own
    let made = st.dependents_of([source]);
    assert_eq!(
        text(&Json::from(st.detail_with(target, &made))),
        text(&Json::from(st.detail(target)))
    );
    // and so is its readiness, however many issues the details were made for
    for asked in [vec![source, other], vec![source], vec![]] {
        let details = st.details_of(asked.iter().copied());
        for issue in [target, other, source] {
            assert_eq!(
                text(&Json::from(details.of(issue))),
                text(&Json::from(st.detail(issue))),
                "{} among {} asked",
                issue.key(),
                asked.len()
            );
        }
    }
    // nor is it given what was found in another state, even of the same issues
    h.link(
        8,
        &other.uid().to_string(),
        &target.uid().to_string(),
        "related",
    );
    let later = h.state();
    let found = later.dependents_of(later.issues());
    assert_eq!(
        text(&Json::from(st.detail_with(target, &found))),
        text(&Json::from(st.detail(target)))
    );
    let target_later = named(&later, "dd-target");
    assert_eq!(
        agree(&later, &[target_later]),
        [[
            "dd-other parent-child",
            "dd-other related",
            "dd-source blocks"
        ]]
    );
    let none = st.dependents_of([]);
    for issue in [target, other, source] {
        assert_eq!(
            text(&Json::from(st.detail_with(issue, &none))),
            text(&Json::from(st.detail(issue)))
        );
    }
}

/// A deterministic generator, one linear congruential step a draw.
struct Dice(u64);

impl Dice {
    fn below(&mut self, n: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) % n as u64) as usize
    }
}

#[test]
fn random_graphs_agree_with_every_detail_read_on_its_own() {
    const KINDS: [&str; 5] = [
        "blocks",
        "related",
        "parent-child",
        "discovered-from",
        "x-custom",
    ];
    // how many dependents every issue lists, over all the graphs: they are not all empty
    let mut listed = 0;
    for seed in 0..60_u64 {
        let mut dice = Dice(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1));
        let mut h = History::new();
        let mut n = 2;
        let mut uids = Vec::new();
        let count = 2 + dice.below(10);
        for at in 0..count {
            // numbers drawn so that uid order differs from the order made
            let number = 10_000 + (dice.below(1_000) as u64) * 100 + at as u64;
            uids.push(h.create(number, &format!("dd-{}", count - at)));
        }
        for _ in 0..dice.below(30) {
            n += 1;
            let from = uids[dice.below(uids.len())].clone();
            let to = if dice.below(10) == 0 {
                id(5_000)
            } else {
                uids[dice.below(uids.len())].clone()
            };
            let kind = KINDS[dice.below(KINDS.len())];
            match dice.below(8) {
                0 => h.unlink(200_000 + n, &from, &to, kind),
                1 => {
                    h.add(200_000 + n, "a", "close", Some(&from), json!({}));
                }
                2 => {
                    h.add(200_000 + n, "a", "delete", Some(&from), json!({}));
                }
                _ => h.link(200_000 + n, &from, &to, kind),
            }
        }
        let st = h.state();
        listed += agree(&st, &every(&st)).iter().map(Vec::len).sum::<usize>();
        let issues = every(&st);
        let asked: Vec<&Issue> = (0..dice.below(2 * issues.len()))
            .map(|_| issues[dice.below(issues.len())])
            .collect();
        agree(&st, &asked);
    }
    assert!(listed > 200, "{listed}");
}
