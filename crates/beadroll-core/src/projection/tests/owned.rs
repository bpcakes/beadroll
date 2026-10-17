//! What every issue answers of who held it (roll-lqjj.40): its claim and generation, the claims
//! that ended, its offline claims, the claims that lost and the ends that came too late, with its
//! detail (what `show --json` prints of it) and the export, as replay answered them while an issue
//! kept its ownership history in the projection's state part. Written on that code (9cbd60a, its
//! digests made again on fd5ae9b once restacked onto it) and green there, so that keeping the history with the texts changes none of it: answer by answer on
//! the journals of three stores bead built ([`ANSWERED`]), and by a digest of the answers on every
//! history the differential tests and the delivery-order property replay and on a world of its own
//! whose machines do little but claim, release, take over, close and reopen ([`next_batch`]); each
//! as replay makes it and as a projection read back from its bytes, its texts read in, makes it.

use crate::id::EventId;
use crate::journal::Journal;
use crate::json::{Format, Object, Value, write};
use crate::replay::replay;
use crate::state::{EndedClaim, LostClaim, Provisional, Stale, State, Texts};

use super::super::{Extended, Projection, project};
use super::awaiting::fnv;
use super::hostile::{HERE, base, event, id, journal, record};
use super::shown::{FIXTURES, fixture, replayed};
use super::{
    MACHINES, TextFile, World, WriteBack, equal, load, read_back, read_in, records, same, str_of,
    whole,
};

/// What `state` answers of each issue's ownership, a line each: its claim, its generation, the
/// claims that ended, its offline claims, the claims that lost, the ends that came too late and its
/// detail; then each export record.
pub(crate) fn answers(state: &State) -> String {
    let json = |value: Value| write(&value, Format::COMPACT).unwrap_or_default();
    let all = |items: Vec<Object>| json(Value::Array(items.into_iter().map(Value::from).collect()));
    let details = state.details_of(state.issues());
    let mut out = String::new();
    for issue in state.issues() {
        let mut say = |what: &str, answer: String| {
            out.push_str(&format!("{}\t{what}\t{answer}\n", issue.key()));
        };
        let claim = (issue.claim()).map_or(Value::Null, |claim| Value::from(claim.to_value()));
        say("claim", json(claim));
        say(
            "generation",
            json(Value::Int(issue.claim_generation().clone())),
        );
        let ended = issue.claim_history().iter().map(EndedClaim::to_value);
        say("ended", all(ended.collect()));
        let offline = issue.provisional_claims().iter().map(Provisional::to_value);
        say("offline", all(offline.collect()));
        let lost = issue.lost_claims().iter().map(LostClaim::to_value);
        say("lost", all(lost.collect()));
        let stale = issue.stale().iter().map(Stale::to_value);
        say("stale", all(stale.collect()));
        say("show", json(Value::from(details.of(issue))));
    }
    for record in state.export_records() {
        out.push_str(&format!("export\t{}\n", json(Value::from(record))));
    }
    out
}

/// Asserts that every issue of `state`, read back with the texts of the issues `read` names read
/// in, answers of who holds it what it does in `original`: its claim, generation and offline
/// claims, and who ended its last claim that ended and how; and those read in their whole
/// ownership history, which those not read in hold none of (roll-lqjj.40).
pub(super) fn read_alike(state: &State, original: &State, read: &[EventId]) {
    for was in original.issues() {
        let issue = state.issue(was.uid()).unwrap();
        let key = issue.key();
        assert_eq!(issue.claim(), was.claim(), "{key}");
        assert_eq!(issue.claim_generation(), was.claim_generation(), "{key}");
        assert_eq!(
            issue.provisional_claims(),
            was.provisional_claims(),
            "{key}"
        );
        assert_eq!(issue.last_ended(), was.last_ended(), "{key}");
        if read.contains(&issue.uid()) {
            assert_eq!(issue.claim_history(), was.claim_history(), "{key}");
            assert_eq!(issue.lost_claims(), was.lost_claims(), "{key}");
            assert_eq!(issue.stale(), was.stale(), "{key}");
        } else {
            let held =
                issue.claim_history().len() + issue.lost_claims().len() + issue.stale().len();
            assert_eq!(held, 0, "{key}: its ownership history is with its texts");
        }
    }
}

/// Where FNV-1a starts.
pub(crate) const BASIS: u64 = 0xcbf2_9ce4_8422_2325;

/// The digest of [`answers`], from [`BASIS`], of the final state of each seed's history, as replay
/// made them on the code that kept the ownership history in the state part (fd5ae9b, with the
/// generator drawing [`World::own`] and [`World::race`]): of the differential tests' histories
/// (`taken::differential`), plain and flawed, and of the delivery-order property's
/// (`replay::tests`), plain and flawed. A change to the generator changes them, the failure
/// printing each: they are then made again on the commit that changes the generator alone
/// (2d44707, made from 5a2e6c7 when restacked onto fd5ae9b, or what a later restack makes
/// of it), never on a later one, whose code would be compared with itself.
pub(crate) const DIFFERENTIAL: u64 = 0x4034_6c5b_1d6d_a79f;
/// See [`DIFFERENTIAL`].
pub(crate) const FLAWED: u64 = 0x83d5_d990_ed33_6b7d;
/// See [`DIFFERENTIAL`].
pub(crate) const DELIVERED: u64 = 0x0f35_b525_33aa_cd8b;
/// See [`DIFFERENTIAL`].
pub(crate) const DELIVERED_FLAWED: u64 = 0x61e0_c5f4_ee54_b08a;
/// See [`DIFFERENTIAL`]: of the world of [`next_batch`].
const OWNED: u64 = 0x3896_9a4f_0069_a36d;

/// What replay answered at 9cbd60a of the issues of the journals of three stores bead built: the
/// golden tracker and the fleet of two machines `shown` pins the fields of ([`FIXTURES`]), and two
/// machines that claim and release with a reason, take over with one while the old owner closes
/// (a close superseded, so ignored), close their own claim, claim offline while another holds the
/// issue and have that owner's close end both, claim offline and then online, take over while the
/// old owner releases offline (a release too late), reopen and close through `update` while
/// claimed, an online claim another machine wrote without seeing the owner's (planted in the store
/// as a third machine's journal: no remote accepts both, so it lost), and an offline claim left
/// as it is.
const ANSWERED: [(&str, &str, &str); 3] = [
    (
        "golden",
        FIXTURES[0].1,
        include_str!("owned/golden.answers"),
    ),
    ("fleet", FIXTURES[1].1, include_str!("owned/fleet.answers")),
    (
        "ownership",
        include_str!("owned/ownership.journal"),
        include_str!("owned/ownership.answers"),
    ),
];

/// Asserts that `got` is `expected`, naming the first line that is not.
fn alike(name: &str, got: &str, expected: &str) {
    if got == expected {
        return;
    }
    let at = (got.lines().zip(expected.lines()))
        .position(|(a, b)| a != b)
        .unwrap_or_else(|| got.lines().count().min(expected.lines().count()));
    panic!(
        "{name} answers otherwise at line {}:\n  answered: {:?}\n  pinned:   {:?}",
        at + 1,
        got.lines().nth(at),
        expected.lines().nth(at)
    );
}

/// `journal`'s state as a projection of it read back from its bytes, its texts read in.
fn read_whole(journal: Journal) -> State {
    let mut file = TextFile::new();
    let read = read_back(&project(journal).unwrap(), &mut file, WriteBack::Encoded);
    whole(&read, &file).into_state()
}

#[test]
fn every_issue_of_the_stores_bead_built_answers_of_its_ownership_what_it_did() {
    for (name, journal, expected) in ANSWERED {
        let state = replay(fixture(journal)).unwrap();
        alike(name, &answers(&state), expected);
        alike(name, &answers(&read_whole(fixture(journal))), expected);
    }
    // the ownership store holds every kind of record the answers pin
    let state = replay(fixture(ANSWERED[2].1)).unwrap();
    let mut ended: Vec<&str> = state
        .issues()
        .flat_map(|issue| issue.claim_history())
        .map(|e| e.how)
        .collect();
    ended.sort_unstable();
    ended.dedup();
    assert_eq!(ended, ["close", "release", "reopen", "takeover"]);
    let stale: Vec<&str> = state
        .issues()
        .flat_map(|issue| issue.stale())
        .map(|s| s.op.as_str())
        .collect();
    assert_eq!(stale, ["close", "release"]);
    assert_eq!(
        state
            .issues()
            .map(|issue| issue.lost_claims().len())
            .sum::<usize>(),
        1
    );
    assert_eq!(
        state
            .issues()
            .map(|issue| issue.provisional_claims().len())
            .sum::<usize>(),
        1
    );
}

#[test]
fn an_event_that_can_add_to_an_issues_history_waits_for_its_texts_and_has_them_written_again() {
    // issue 2 claimed and the claim released: a claim that ended, in its history
    let mut records = base();
    let mut online = Object::new();
    online.insert("mode".into(), Value::from("online"));
    records.push(record(
        event(4, HERE, &[3], "claim", 2, online.clone()),
        HERE,
        true,
    ));
    let mut naming = Object::new();
    naming.insert("claim".into(), Value::from(id(4)));
    records.push(record(
        event(5, HERE, &[4], "release", 2, naming.clone()),
        HERE,
        true,
    ));
    let (encoded, _) = project(journal(&records)).unwrap().encode(0);
    let read = Projection::decode(&encoded).unwrap();
    let uid = EventId::parse(&id(2)).unwrap();
    // read back, its history is with its texts, and who ended the claim and how is not
    let issue = read.state().issue(uid).unwrap();
    assert!(issue.claim_history().is_empty());
    let ended = issue.last_ended().map(|ended| ended.how);
    assert_eq!(ended, Some("release"));
    let mut closing = naming.clone();
    closing.insert("reason".into(), Value::from("done"));
    // whether each adds to the history: a claim and a reopen of no claim do not, a release and a
    // close naming the claim that ended come too late, and a delete changes the extras too
    for (op, data, adds) in [
        ("claim", online, false),
        ("reopen", Object::new(), false),
        ("release", naming, true),
        ("close", closing, true),
        ("delete", Object::new(), true),
    ] {
        let batch = vec![record(event(6, HERE, &[5], op, 2, data), HERE, true)];
        let declined = read.clone().extend(batch.clone()).unwrap();
        assert!(matches!(declined, Extended::Replay), "{op}");
        let mut texts = read.clone();
        let file = encoded.texts.clone();
        let about = texts.about(&batch);
        assert_eq!(about, [uid], "{op}");
        read_in(texts.state_mut(), &file, Some(about));
        let Extended::Applied(extended) = texts.extend(batch.clone()).unwrap() else {
            panic!("{op}: its texts were read in");
        };
        let mut all = records.clone();
        all.extend(batch);
        assert!(same(extended.state(), &replayed(&all)), "{op}");
        let issue = extended.state().issue(uid).unwrap();
        assert_eq!(matches!(issue.texts(), Texts::Here), adds, "{op}");
        // the history added to is written again with the texts, and read back as replay made it,
        // whichever way it is written back
        for back in WriteBack::EVERY {
            read_back(&extended, &mut file.clone(), back);
        }
    }
}

/// What histories hold of who held each issue, counted in what replay made of them: the claims
/// that ended, by how, and the takeovers among them that gave a reason; the claims that lost and
/// the ends that came too late, by what they were; the offline claims still standing, and those
/// their owner settled later, by claiming online or releasing; and the batches a projection read
/// back took in that added to the history of an issue it had read in.
#[derive(Debug, Default)]
pub(super) struct Seen {
    ended: std::collections::BTreeMap<String, usize>,
    reasons: usize,
    lost: usize,
    stale: std::collections::BTreeMap<String, usize>,
    offline: usize,
    settled: usize,
    pub(super) appended: usize,
}

impl Seen {
    /// Whether more than `n` of each were seen.
    pub(super) fn more_than(&self, n: usize) -> bool {
        let every = |counts: &std::collections::BTreeMap<_, usize>, kinds: &[&str]| {
            (kinds.iter()).all(|kind| counts.iter().any(|(k, c)| k == kind && *c > n))
        };
        every(
            &self.ended,
            &["close", "delete", "release", "reopen", "takeover"],
        ) && every(&self.stale, &["close", "delete", "release"])
            && [
                self.reasons,
                self.lost,
                self.offline,
                self.settled,
                self.appended,
            ]
            .iter()
            .all(|count| *count > n)
    }

    /// Counts what `state`, replayed from `world`'s events, holds.
    pub(super) fn count(&mut self, world: &World, state: &State) {
        for issue in state.issues() {
            for ended in issue.claim_history() {
                *self.ended.entry(ended.how.to_owned()).or_default() += 1;
                self.reasons += usize::from(ended.how == "takeover" && ended.reason.is_some());
            }
            self.lost += issue.lost_claims().len();
            for stale in issue.stale() {
                *self.stale.entry(stale.op.clone()).or_default() += 1;
            }
            self.offline += issue.provisional_claims().len();
        }
        // an offline claim, then a claim online or a release by the same writer on its issue
        let writer = |value: &Value| {
            let thread = value.get("actor").and_then(|actor| actor.get("thread"));
            (
                str_of(value.get("replica")),
                str_of(thread),
                str_of(value.get("issue")),
            )
        };
        for (at, written) in world.events.iter().enumerate() {
            let data = written.value.get("data");
            let mode = data
                .and_then(|data| data.get("mode"))
                .and_then(Value::as_str);
            if mode != Some("offline") {
                continue;
            }
            let by = writer(&written.value);
            self.settled += usize::from(world.events[at + 1..].iter().any(|later| {
                let op = later.value.get("op").and_then(Value::as_str);
                let mode = (later.value.get("data"))
                    .and_then(|data| data.get("mode"))
                    .and_then(Value::as_str);
                writer(&later.value) == by
                    && (op == Some("release") || (op == Some("claim") && mode == Some("online")))
            }));
        }
    }
}

/// How long each issue's history is (its ended claims, lost claims and late ends), of those `uids`
/// names.
pub(super) fn histories(state: &State, uids: &[EventId]) -> Vec<(EventId, usize)> {
    (uids.iter())
        .filter_map(|uid| state.issue(*uid))
        .map(|issue| {
            let long =
                issue.claim_history().len() + issue.lost_claims().len() + issue.stale().len();
            (issue.uid(), long)
        })
        .collect()
}

/// Whether an issue of `before` whose history was not empty has a longer one in `after`.
pub(super) fn added(before: &[(EventId, usize)], after: &State) -> bool {
    let uids: Vec<EventId> = before.iter().map(|(uid, _)| *uid).collect();
    let after = histories(after, &uids);
    (before.iter().zip(&after)).any(|((_, was), (_, now))| *was > 0 && now > was)
}

/// Seeds of the world of [`next_batch`], and the batches machine 0's store takes in of each.
const SEEDS: u64 = 24;
const BATCHES: usize = 50;

#[test]
fn a_world_heavy_in_ownership_answers_what_it_did_replayed_extended_and_read_back() {
    let (mut digest, mut seen) = (BASIS, Seen::default());
    for seed in 0..SEEDS {
        let mut world = World::new(20_261_017 + seed);
        let mut data = Object::new();
        data.insert("prefix".into(), Value::from("pr"));
        world.write(0, "init", None, data);
        let mut held: Vec<usize> = vec![0];
        let first = project(load(&world, &held)).unwrap();
        // each way of writing back (roll-lqjj.43): its projection, its text file, and whether the
        // projection was read back since replay last made it
        let mut tracks = WriteBack::EVERY.map(|back| (first.clone(), TextFile::new(), back, false));
        for _ in 0..BATCHES {
            let batch = next_batch(&mut world);
            let reading_back = world.rng.chance(50);
            held.extend(&batch);
            let full = project(load(&world, &held)).unwrap();
            let records = records(&world, &batch);
            for (projection, file, back, read) in &mut tracks {
                if reading_back {
                    *projection = read_back(projection, file, *back);
                    *read = true;
                }
                let about = projection.about(&records);
                read_in(projection.state_mut(), file, Some(about.clone()));
                let before = histories(projection.state(), &about);
                *projection = match projection.clone().extend(records.clone()).unwrap() {
                    Extended::Applied(extended) => {
                        assert!(
                            equal(&whole(&extended, file), &full),
                            "seed {seed}, {back:?}"
                        );
                        // the figures are the store's
                        let counted = *back == WriteBack::Encoded;
                        let added = added(&before, extended.state());
                        seen.appended += usize::from(counted && *read && added);
                        *extended
                    }
                    Extended::Replay => {
                        *read = false;
                        full.clone()
                    }
                };
            }
        }
        let state = replay(load(&world, &held)).unwrap();
        let said = answers(&state);
        for (projection, file, back, _) in &mut tracks {
            let read = whole(&read_back(projection, file, *back), file);
            alike(
                &format!("seed {seed} read back, {back:?}"),
                &answers(read.state()),
                &said,
            );
        }
        digest = fnv(digest, said.as_bytes());
        seen.count(&world, &state);
    }
    assert!(seen.more_than(50), "{seen:?}");
    assert_eq!(digest, OWNED, "{digest:#x}");
}

/// What machine 0's store of a world heavy in ownership takes in next, a local write's events or
/// a merge's, after the other machines have done what they do meanwhile, two of them now and then
/// claiming one issue at once.
fn next_batch(world: &mut World) -> Vec<usize> {
    loop {
        let m = world.rng.below(MACHINES.len());
        if m == 0 && world.rng.chance(60) {
            let before = world.events.len();
            for _ in 0..1 + world.rng.below(3) {
                act(world, 0);
            }
            return (before..world.events.len()).collect();
        }
        if m == 0 {
            return world.sync(0);
        }
        if world.rng.chance(15) {
            world.race();
            continue;
        }
        if world.rng.chance(70) {
            world.sync(m);
        }
        for _ in 0..1 + world.rng.below(3) {
            act(world, m);
        }
        world.sync(m);
    }
}

/// One thing machine `m` of a world heavy in ownership does: now and then a create, or a comment or
/// a description, which change an issue's texts; mostly something about who holds an issue.
fn act(world: &mut World, m: usize) {
    let (issues, _) = world.issues(m);
    let roll = world.rng.below(100);
    let issue = world.rng.pick(&issues).cloned();
    let Some(issue) = issue.filter(|_| roll >= 10) else {
        let mut fields = Object::new();
        fields.insert("title".into(), Value::from(format!("t{roll}")));
        fields.insert("status".into(), Value::from("open"));
        let mut data = Object::new();
        let key = ["a1", "b2", "c3", "d4", "e5", "f6"][world.rng.below(6)];
        data.insert("key".into(), Value::from(format!("pr-{key}")));
        data.insert("fields".into(), Value::from(fields));
        world.write(m, "create", None, data);
        return;
    };
    let mut data = Object::new();
    match roll {
        10..=15 => {
            data.insert("text".into(), Value::from(format!("c{roll}")));
            world.write(m, "comment", Some(issue), data);
        }
        16..=19 => {
            let mut fields = Object::new();
            fields.insert("description".into(), Value::from(format!("d{roll}")));
            data.insert("fields".into(), Value::from(fields));
            world.write(m, "set", Some(issue), data);
        }
        _ => world.own(m, issue),
    }
}

/// A claim a machine knows of: its id, and the machine and thread that made it.
struct Known {
    id: String,
    machine: usize,
    thread: String,
}

impl World {
    /// The claims machine `m` knows of on `issue`, in the order it learnt of them.
    fn claims_on(&self, m: usize, issue: &str) -> Vec<Known> {
        (self.machines[m].known.iter())
            .filter_map(|at| {
                let written = &self.events[*at];
                let value = &written.value;
                let claim = value.get("op").and_then(Value::as_str) == Some("claim");
                (claim && str_of(value.get("issue")) == issue).then(|| Known {
                    id: self.id_of(*at),
                    machine: written.machine,
                    thread: str_of(value.get("actor").and_then(|actor| actor.get("thread"))),
                })
            })
            .collect()
    }

    /// One thing machine `m` does about who holds `issue`: a claim online or offline; a takeover of
    /// a claim it knows of, with a reason; a release, close or delete naming a claim of its own by
    /// the thread that made it (its owner's, when that claim is still current, and an end too late
    /// when a takeover superseded it), or naming none (a release of an offline claim, a close or
    /// delete that ends whatever claim its writer had seen); or a reopen.
    pub(super) fn own(&mut self, m: usize, issue: String) {
        let known = self.claims_on(m, &issue);
        let mine: Vec<&Known> = known.iter().filter(|known| known.machine == m).collect();
        let roll = self.rng.below(100);
        let ended = self
            .rng
            .pick(&mine)
            .map(|own| (own.id.clone(), own.thread.clone()));
        let taken = self.rng.pick(&known).map(|other| other.id.clone());
        let mut data = Object::new();
        let (op, thread) = match roll {
            0..=21 => {
                data.insert("mode".into(), Value::from("online"));
                ("claim", None)
            }
            22..=33 => {
                data.insert("mode".into(), Value::from("offline"));
                ("claim", None)
            }
            34..=43 => {
                data.insert("mode".into(), Value::from("online"));
                if let Some(taken) = taken {
                    data.insert("takeover".into(), Value::from(taken));
                    let reason = ["gone", "stale", "handed over"][roll % 3];
                    data.insert("reason".into(), Value::from(reason));
                }
                ("claim", None)
            }
            44..=85 => {
                let op = match roll {
                    44..=61 => "release",
                    62..=79 => "close",
                    _ => "delete",
                };
                if op != "release" {
                    data.insert("reason".into(), Value::from("done"));
                } else if roll.is_multiple_of(2) {
                    data.insert("reason".into(), Value::from("for now"));
                }
                match ended.filter(|_| op == "release" || !roll.is_multiple_of(3)) {
                    Some((claim, thread)) => {
                        data.insert("claim".into(), Value::from(claim));
                        (op, Some(thread))
                    }
                    None => (op, None),
                }
            }
            _ => ("reopen", None),
        };
        self.write_as(m, thread.as_deref(), op, Some(issue), data);
    }

    /// Machines 1 and 2 each claim an issue both know of, online, neither having seen the other's
    /// claim, and publish: the one replay applies second loses to the first.
    pub(super) fn race(&mut self) {
        let (one, _) = self.issues(1);
        let (two, _) = self.issues(2);
        let both: Vec<String> = one
            .into_iter()
            .filter(|issue| two.contains(issue))
            .collect();
        let Some(issue) = self.rng.pick(&both).cloned() else {
            return;
        };
        for m in [1, 2] {
            let mut data = Object::new();
            data.insert("mode".into(), Value::from("online"));
            self.write(m, "claim", Some(issue.clone()), data);
        }
        self.sync(1);
        self.sync(2);
    }
}
