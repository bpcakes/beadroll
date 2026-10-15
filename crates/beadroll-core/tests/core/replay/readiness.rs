//! `State::readiness`, every issue judged at once (roll-lqjj.19), against the blocker lists
//! `State::open_blockers` builds issue by issue, and `State::is_blocked`, the same question asked of
//! one issue: deep chains, shared and second parents, merged parent cycles, done and disputed
//! ancestors and targets, epics beside plain parents, claims, blockers outside a scoped subtree, and
//! random graphs. `State::ready` keeps its order and `open_blockers` its lists.

use std::collections::BTreeSet;

use beadroll_core::event::{DONE, Field};
use beadroll_core::id::EventId;
use beadroll_core::query::{ReadyScope, sort_for_listing};
use beadroll_core::state::{Issue, State};
use beadroll_test_support::{Json, json};

use super::replay_audit_properties::fixture::{Fixture, fields, issue};

/// Adds a dependency of `kind` from `from` to `to` after the last event.
fn link(f: &mut Fixture, from: &str, to: &str, kind: &str) {
    let last = f.last();
    f.event(
        "dep_add",
        Some(from),
        "seed",
        &[&last],
        json!({"to": to, "type": kind}),
    );
}

/// Makes `child` a child of `parent`.
fn adopt(f: &mut Fixture, child: &str, parent: &str) {
    link(f, child, parent, "parent-child");
}

/// Makes `issue` wait for `prerequisite`.
fn wait(f: &mut Fixture, issue: &str, prerequisite: &str) {
    link(f, issue, prerequisite, "blocks");
}

/// Does `op` to `uid` after the last event, on `seed`.
fn apply(f: &mut Fixture, op: &str, uid: &str, data: Json) {
    let last = f.last();
    f.event(op, Some(uid), "seed", &[&last], data);
}

/// Sets the status of `uid`.
fn status(f: &mut Fixture, uid: &str, status: &str) {
    apply(f, "set", uid, fields(Field::Status, json!(status)));
}

/// Creates an epic.
fn epic(f: &mut Fixture, key: &str) -> String {
    let last = f.last();
    f.event(
        "create",
        None,
        "seed",
        &[&last],
        json!({"key": key, "fields": {"title": key, "issue_type": "epic"}}),
    )
}

/// Disputes the status of `uid`: two machines of their own (named after `tag`, since a machine's
/// events must each build on the one before) close and delete it without seeing each other, so the
/// issue shows the tombstone while its register holds both: two done statuses that still do not
/// make it done.
fn dispute(f: &mut Fixture, uid: &str, tag: &str) {
    let last = f.last();
    f.event("close", Some(uid), &format!("a-{tag}"), &[&last], json!({}));
    f.event(
        "delete",
        Some(uid),
        &format!("b-{tag}"),
        &[&last],
        json!({}),
    );
}

fn uid(text: &str) -> EventId {
    EventId::parse(text).unwrap()
}

fn keys(issues: &[&Issue]) -> Vec<String> {
    issues.iter().map(|i| i.key().to_owned()).collect()
}

/// What `ready` must list by the blocker lists: open, unclaimed, undisputed issues whose list is
/// empty, inside `beneath` when a scope is given, in listing order.
fn frontier<'s>(st: &'s State, beneath: Option<&BTreeSet<EventId>>) -> Vec<&'s Issue> {
    let mut items: Vec<&Issue> = st
        .issues()
        .filter(|i| i.status() == "open" && i.claim().is_none())
        .filter(|i| !st.disputed(i, Field::Status) && st.open_blockers(i).is_empty())
        .filter(|i| beneath.is_none_or(|beneath| beneath.contains(&i.uid())))
        .collect();
    sort_for_listing(&mut items);
    items
}

/// Every issue's answer judged with every other and asked on its own against its blocker list, and
/// `ready` against the same filter over the lists; the keys of the issues held, in replay order.
fn agree(st: &State) -> Vec<String> {
    let readiness = st.readiness();
    let mut held = Vec::new();
    for issue in st.issues() {
        let listed = !st.open_blockers(issue).is_empty();
        assert_eq!(
            st.is_blocked(issue),
            listed,
            "{} asked on its own",
            issue.key()
        );
        assert_eq!(
            readiness.is_blocked(issue),
            listed,
            "{} judged with every issue",
            issue.key()
        );
        if listed {
            held.push(issue.key().to_owned());
        }
    }
    assert_eq!(keys(&st.ready(ReadyScope::All)), keys(&frontier(st, None)));
    held
}

/// `ready` inside `root`'s subtree against the same filter over the lists, restricted to the
/// subtree.
fn agree_beneath(st: &State, root: &str) -> Vec<String> {
    let beneath = st.descendants(uid(root));
    let scoped = st.ready(ReadyScope::Subtree(uid(root)));
    assert_eq!(keys(&scoped), keys(&frontier(st, Some(&beneath))));
    keys(&scoped)
}

/// A chain of `depth` issues, each the child of the one before, under the audit's `au-one`-free
/// root `au-0`; the uids, top first.
fn chain(f: &mut Fixture, depth: usize) -> Vec<String> {
    let mut chain = vec![f.create("au-0")];
    for level in 1..depth {
        let child = f.create(&format!("au-{level}"));
        adopt(f, &child, &chain[level - 1]);
        chain.push(child);
    }
    chain
}

#[test]
fn a_chain_is_held_by_its_roots_prerequisite_until_a_done_ancestor_cuts_it_off() {
    let mut f = Fixture::new();
    let gate = f.issue.clone();
    let chain = chain(&mut f, 2_000);
    // plain parents wait for nothing, and nothing holds a chain of them
    assert!(agree(&f.state()).is_empty());
    wait(&mut f, &chain[0], &gate);
    let st = f.state();
    let held = agree(&st);
    assert_eq!(held.len(), 2_000, "every issue of the chain, not the gate");
    assert!(!held.contains(&"au-one".to_owned()));
    assert_eq!(keys(&st.ready(ReadyScope::All)), ["au-one"]);
    // the gate in progress, and held: not done, so it still holds
    status(&mut f, &gate, "in_progress");
    apply(
        &mut f,
        "claim",
        &gate,
        json!({"mode": "online", "generation": 1}),
    );
    assert_eq!(agree(&f.state()).len(), 2_000);
    // a done ancestor's prerequisites are not inherited, and the walk does not pass it: the issues
    // beneath it are free, the ones above it and it itself still held through its own parent
    apply(&mut f, "close", &chain[1_000], json!({}));
    let st = f.state();
    let held = agree(&st);
    assert_eq!(held.len(), 1_001, "au-0 to au-1000");
    assert!(held.contains(&"au-1000".to_owned()));
    assert!(!held.contains(&"au-1001".to_owned()));
    assert_eq!(st.ready(ReadyScope::All).len(), 999, "au-1001 to au-1999");
    // the gate closes: nothing holds anything
    apply(&mut f, "close", &gate, json!({}));
    assert!(agree(&f.state()).is_empty());
}

#[test]
fn a_chain_far_deeper_than_a_thread_stack_takes_is_judged_without_recursion() {
    let mut f = Fixture::new();
    let gate = f.issue.clone();
    let chain = chain(&mut f, 20_000);
    wait(&mut f, &chain[0], &gate);
    let st = f.state();
    let readiness = st.readiness();
    assert_eq!(
        st.issues().filter(|i| readiness.is_blocked(i)).count(),
        20_000
    );
    assert!(st.is_blocked(issue(&st, &chain[19_999])));
    assert_eq!(keys(&st.ready(ReadyScope::All)), ["au-one"]);
    assert_eq!(
        keys(&st.open_blockers(issue(&st, &chain[19_999]))),
        ["au-one"]
    );
}

#[test]
fn second_and_shared_parents_hold_through_whichever_of_them_is_held() {
    let mut f = Fixture::new();
    let gate = f.issue.clone();
    let free = f.create("au-free-parent");
    let held = f.create("au-held-parent");
    let child = f.create("au-child");
    adopt(&mut f, &child, &free);
    adopt(&mut f, &child, &held);
    let grandchild = f.create("au-grandchild");
    adopt(&mut f, &grandchild, &child);
    let sibling = f.create("au-sibling");
    adopt(&mut f, &sibling, &held);
    assert!(agree(&f.state()).is_empty());
    wait(&mut f, &held, &gate);
    assert_eq!(
        agree(&f.state()),
        ["au-held-parent", "au-child", "au-grandchild", "au-sibling"]
    );
    // the held parent closed: its own prerequisite still lists it as held, but a done parent's
    // prerequisite is not inherited, and its status alone never was what held the others
    apply(&mut f, "close", &held, json!({}));
    assert_eq!(agree(&f.state()), ["au-held-parent"]);
    apply(&mut f, "reopen", &held, json!({}));
    status(&mut f, &held, "deferred");
    assert_eq!(
        agree(&f.state()),
        ["au-held-parent", "au-child", "au-grandchild", "au-sibling"]
    );
}

#[test]
fn a_merged_parent_cycle_ends_where_it_leads_back_and_a_self_parent_holds_nothing() {
    let mut f = Fixture::new();
    let gate = f.issue.clone();
    let k = f.create("au-k");
    let m = f.create("au-m");
    let n = f.create("au-n");
    let z = f.create("au-z");
    adopt(&mut f, &m, &k);
    adopt(&mut f, &n, &m);
    adopt(&mut f, &k, &n);
    adopt(&mut f, &z, &n);
    assert!(agree(&f.state()).is_empty());
    wait(&mut f, &m, &gate);
    assert_eq!(agree(&f.state()), ["au-k", "au-m", "au-n", "au-z"]);
    // the loop broken by a done member: what reaches the prerequisite only through it is free,
    // while the done member itself is still held through its own parent
    apply(&mut f, "close", &n, json!({}));
    assert_eq!(agree(&f.state()), ["au-m", "au-n"]);
    apply(&mut f, "close", &m, json!({}));
    assert_eq!(agree(&f.state()), ["au-m"], "held by its own prerequisite");
    // an issue that is its own parent
    let own = f.create("au-own");
    adopt(&mut f, &own, &own);
    let beneath = f.create("au-beneath-own");
    adopt(&mut f, &beneath, &own);
    assert_eq!(agree(&f.state()), ["au-m"]);
    wait(&mut f, &own, &gate);
    assert_eq!(agree(&f.state()), ["au-m", "au-own", "au-beneath-own"]);
}

#[test]
fn done_and_disputed_statuses_count_as_the_lists_count_them() {
    let mut f = Fixture::new();
    let gate = f.issue.clone();
    let closed = f.create("au-closed");
    apply(&mut f, "close", &closed, json!({}));
    let deleted = f.create("au-deleted");
    apply(&mut f, "delete", &deleted, json!({}));
    let deferred = f.create("au-deferred");
    status(&mut f, &deferred, "deferred");
    let started = f.create("au-started");
    status(&mut f, &started, "in_progress");
    let disputed = f.create("au-disputed");
    dispute(&mut f, &disputed, "target");
    let mut waiting = Vec::new();
    for (name, target) in [
        ("closed", &closed),
        ("deleted", &deleted),
        ("deferred", &deferred),
        ("started", &started),
        ("disputed", &disputed),
    ] {
        let issue = f.create(&format!("au-waits-{name}"));
        wait(&mut f, &issue, target);
        waiting.push(issue);
    }
    // a done parent with a prerequisite, and one whose done status is disputed
    let done_parent = f.create("au-done-parent");
    wait(&mut f, &done_parent, &gate);
    apply(&mut f, "close", &done_parent, json!({}));
    let under_done = f.create("au-under-done");
    adopt(&mut f, &under_done, &done_parent);
    let disputed_parent = f.create("au-disputed-parent");
    wait(&mut f, &disputed_parent, &gate);
    dispute(&mut f, &disputed_parent, "parent");
    let under_disputed = f.create("au-under-disputed");
    adopt(&mut f, &under_disputed, &disputed_parent);
    let st = f.state();
    assert_eq!(issue(&st, &deferred).status(), "deferred");
    assert_eq!(issue(&st, &started).status(), "in_progress");
    for uid in [&disputed, &disputed_parent] {
        let issue = issue(&st, uid);
        assert!(DONE.contains(&issue.status()), "{}", issue.status());
        assert!(st.disputed(issue, Field::Status));
    }
    assert_eq!(
        agree(&st),
        [
            "au-waits-deferred",
            "au-waits-started",
            "au-waits-disputed",
            "au-done-parent",
            "au-disputed-parent",
            "au-under-disputed",
        ]
    );
}

#[test]
fn only_an_epic_waits_for_its_children_and_nothing_inherits_that_wait() {
    let mut f = Fixture::new();
    let epic_ = epic(&mut f, "au-epic");
    let child = f.create("au-child");
    adopt(&mut f, &child, &epic_);
    let grandchild = f.create("au-grandchild");
    adopt(&mut f, &grandchild, &child);
    let plain = f.create("au-plain");
    let under_plain = f.create("au-under-plain");
    adopt(&mut f, &under_plain, &plain);
    let nested = epic(&mut f, "au-nested-epic");
    adopt(&mut f, &nested, &epic_);
    let under_nested = f.create("au-under-nested");
    adopt(&mut f, &under_nested, &nested);
    assert_eq!(agree(&f.state()), ["au-epic", "au-nested-epic"]);
    // children done, deferred and deleted: only an unfinished one counts
    apply(&mut f, "close", &child, json!({}));
    assert_eq!(agree(&f.state()), ["au-epic", "au-nested-epic"]);
    apply(&mut f, "close", &under_nested, json!({}));
    assert_eq!(
        agree(&f.state()),
        ["au-epic"],
        "the nested epic is still open"
    );
    apply(&mut f, "delete", &nested, json!({}));
    assert!(agree(&f.state()).is_empty());
    status(&mut f, &child, "deferred");
    assert_eq!(agree(&f.state()), ["au-epic"]);
}

#[test]
fn claims_and_a_scope_restrict_what_is_listed_and_never_what_holds() {
    let mut f = Fixture::new();
    let gate = f.issue.clone();
    let root = f.create("au-root");
    let inside = f.create("au-inside");
    adopt(&mut f, &inside, &root);
    let deeper = f.create("au-deeper");
    adopt(&mut f, &deeper, &inside);
    let urgent = f.create("au-urgent");
    adopt(&mut f, &urgent, &root);
    apply(&mut f, "set", &urgent, fields(Field::Priority, json!(0)));
    let outsider = f.create("au-outsider");
    let twice = f.create("au-twice");
    adopt(&mut f, &twice, &root);
    adopt(&mut f, &twice, &outsider);
    let st = f.state();
    assert!(agree(&st).is_empty());
    assert_eq!(
        agree_beneath(&st, &root),
        ["au-urgent", "au-inside", "au-deeper", "au-twice"],
        "priority first, then creation, the root never among them"
    );
    // a prerequisite outside the scope holds an issue inside it, through its parent and through a
    // second parent, and a claimed issue is left out while the walk goes on beneath it
    wait(&mut f, &inside, &gate);
    wait(&mut f, &outsider, &gate);
    apply(
        &mut f,
        "claim",
        &urgent,
        json!({"mode": "online", "generation": 1}),
    );
    let st = f.state();
    assert_eq!(
        agree(&st),
        ["au-inside", "au-deeper", "au-outsider", "au-twice"]
    );
    assert!(issue(&st, &urgent).claim().is_some());
    assert!(agree_beneath(&st, &root).is_empty());
    assert!(agree_beneath(&st, &inside).is_empty());
    apply(&mut f, "close", &gate, json!({}));
    let st = f.state();
    assert_eq!(
        agree_beneath(&st, &root),
        ["au-inside", "au-deeper", "au-twice"]
    );
    assert_eq!(agree_beneath(&st, &inside), ["au-deeper"]);
}

/// A deterministic generator, one linear congruential step a draw, so each seed's graph is the
/// same on every run.
struct Dice(u64);

impl Dice {
    fn below(&mut self, n: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) as usize) % n
    }
}

#[test]
fn random_graphs_agree_with_the_blocker_lists() {
    let mut ever_held = 0;
    for seed in 0..80_u64 {
        let mut dice = Dice(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1));
        let mut f = Fixture::new();
        let mut uids = vec![f.issue.clone()];
        for n in 1..30 {
            let key = format!("au-{n}");
            uids.push(if dice.below(4) == 0 {
                epic(&mut f, &key)
            } else {
                f.create(&key)
            });
        }
        let pick = |dice: &mut Dice, uids: &[String]| uids[dice.below(uids.len())].clone();
        for from in uids.clone() {
            for _ in 0..dice.below(3) {
                let to = pick(&mut dice, &uids);
                adopt(&mut f, &from, &to);
            }
            if dice.below(3) == 0 {
                let to = pick(&mut dice, &uids);
                wait(&mut f, &from, &to);
            }
        }
        for (n, uid_) in uids.clone().iter().enumerate() {
            match dice.below(9) {
                0 => apply(&mut f, "close", uid_, json!({})),
                1 => apply(&mut f, "delete", uid_, json!({})),
                2 => status(&mut f, uid_, "deferred"),
                3 => status(&mut f, uid_, "in_progress"),
                4 => dispute(&mut f, uid_, &n.to_string()),
                5 => apply(
                    &mut f,
                    "claim",
                    uid_,
                    json!({"mode": "online", "generation": 1}),
                ),
                _ => {}
            }
        }
        let st = f.state();
        ever_held += agree(&st).len();
        let root = pick(&mut dice, &uids);
        agree_beneath(&st, &root);
    }
    assert!(ever_held > 200, "the graphs hold something: {ever_held}");
}
