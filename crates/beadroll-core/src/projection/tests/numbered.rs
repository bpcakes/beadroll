//! Copies of numbered keys two machines minted alike (roll-lqjj.46): every way a state is made
//! renames them to the keys the rule picks one number at a time (`key::free_key`), the first
//! number at or above the copy's own whose key no issue has and no import waits for — replay of
//! the whole journal, a projection extended a record at a time (read back from its bytes between
//! records, which starts with no runs learned, or never read back, which keeps them), and one
//! extended by everything in one batch. The scenarios expect what that rule picks; the seeded
//! histories' digest is what replay made of them before copies skipped runs, at 3ae3604. In this
//! crate's tests every key `State::free_key` picks is also checked against the rule
//! (`state::runs`).

use crate::replay::replay;
use crate::state::{Rename, State};

use super::super::{Extended, Record, project};
use super::awaiting::{CHAINED, THERE, asks_making, at, base, by_key, creating, every_way, on};
use super::hostile::{HERE, journal};

/// The largest number an `i128` holds, and the one below it.
const MOST: &str = "170141183460469231731687303715884105727";
const BELOW: &str = "170141183460469231731687303715884105726";

/// Every rename, as the key both minted and the key the copy has.
fn renames(state: &State) -> Vec<(String, String)> {
    (state.renames().iter())
        .map(|Rename { from, to, .. }| (from.clone(), to.clone()))
        .collect()
}

/// `(from, to)` pairs, as [`renames`] gives them.
fn pairs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    (pairs.iter())
        .map(|(from, to)| ((*from).to_owned(), (*to).to_owned()))
        .collect()
}

/// A create under `key`, numbered `n`, by `replica`, after `after`.
fn make(n: u128, replica: &str, after: u128, key: &str) -> Record {
    at(n, replica, &[after], "create", n, creating(key, vec![]))
}

/// One machine's creates under `keys`, from event `first` on, each built on the one before it and
/// the first on `after`.
fn chain(replica: &str, first: u128, after: u128, keys: &[&str]) -> Vec<Record> {
    (keys.iter().zip(first..))
        .map(|(key, n)| make(n, replica, if n == first { after } else { n - 1 }, key))
        .collect()
}

/// The keys [`State::claims_key`] is asked of while `base` and then `rest` are replayed, each with
/// whether it is claimed, once a projection of `base` taking `rest` in one batch asked the same.
fn asked_of(rest: &[Record]) -> Vec<(String, bool)> {
    let mut all = base();
    all.extend_from_slice(rest);
    let replayed = asks_making(|| replay(journal(&all)).unwrap());
    let in_one_batch =
        asks_making(
            || match project(journal(&base())).unwrap().extend(rest.to_vec()) {
                Ok(Extended::Applied(extended)) => extended.state().clone(),
                _ => panic!("a projection takes them in one batch"),
            },
        );
    assert_eq!(replayed, in_one_batch);
    (replayed.into_iter())
        .map(|(key, claimed, _, _)| (key, claimed))
        .collect()
}

/// Keys asked of, `+` claimed and `-` not, as [`asked_of`] gives them.
fn asks(asks: &[&str]) -> Vec<(String, bool)> {
    (asks.iter())
        .map(|ask| (format!("pr-k.{}", &ask[1..]), ask.starts_with('+')))
        .collect()
}

#[test]
fn a_copy_counts_past_the_numbers_taken_and_a_later_one_can_take_a_lower_gap() {
    // One machine has 1 to 3 and 5 to 7; the other's copies apply after them. A copy of 5 takes 8;
    // a later copy of 1 takes the gap at 4, below what any copy took before it; a copy of 2 then
    // counts past both runs, now one, to 9; one of 7 carries into two digits.
    let mut rest = chain(HERE, 3, 2, &["pr-k.1", "pr-k.2", "pr-k.3"]);
    rest.extend(chain(HERE, 6, 5, &["pr-k.5", "pr-k.6", "pr-k.7"]));
    let copies = ["pr-k.5", "pr-k.1", "pr-k.2", "pr-k.7", "pr-k.3"];
    rest.extend(chain(THERE, 9, 2, &copies));
    let (state, taken) = every_way(&base(), &rest);
    assert!(taken.in_one_batch);
    assert_eq!(
        renames(&state),
        pairs(&[
            ("pr-k.5", "pr-k.8"),
            ("pr-k.1", "pr-k.4"),
            ("pr-k.2", "pr-k.9"),
            ("pr-k.7", "pr-k.10"),
            ("pr-k.3", "pr-k.11"),
        ])
    );
    // The first two copies ask of every number they count past, which joins runs of 5 to 7 and of
    // 1 to 3. Each copy after them asks only of the number after the run it starts in: the copy of
    // 2 of 4, taken by the copy before it, which joins both runs into one, and so of 8 next.
    let first = ["+5", "+6", "+7", "-8", "+1", "+2", "+3", "-4"];
    let after = ["+4", "+8", "-9", "+9", "-10", "+10", "-11"];
    assert_eq!(asked_of(&rest), asks(&[&first[..], &after[..]].concat()));
}

#[test]
fn stems_count_apart_and_a_copy_counts_from_its_number_without_leading_zeros() {
    // `pr-k.1.1` counts under the stem `pr-k.1`, which is itself a key under `pr-k`. A copy of
    // `pr-k.08` counts from 8, and `pr-k.08` does not hold `pr-k.8`; `pr-k.000` counts from 0.
    // A key whose stem has a newline has no number, and its copy takes a prefix of its digest.
    let mine = [
        "pr-k.1",
        "pr-k.1.1",
        "pr-k.10",
        "pr-k.08",
        "pr-k.000",
        "pr-k.007",
        "pr-j.9",
        "pr-a\nb.5",
    ];
    let mut rest = chain(HERE, 3, 2, &mine);
    let copies = [
        "pr-a\nb.5",
        "pr-k.1",
        "pr-k.1.1",
        "pr-k.08",
        "pr-k.000",
        "pr-k.007",
        "pr-k.007",
        "pr-k.000",
        "pr-j.9",
        "pr-k.10",
        "pr-k.1",
    ];
    rest.extend(chain(THERE, 11, 2, &copies));
    let (state, taken) = every_way(&base(), &rest);
    assert!(taken.in_one_batch);
    let mut renamed = renames(&state);
    let (from, digested) = renamed.remove(0);
    assert_eq!(from, "pr-a\nb.5");
    assert!(digested.starts_with("pr-a\nb.5") && digested.len() == from.len() + 1);
    assert_eq!(
        renamed,
        pairs(&[
            ("pr-k.1", "pr-k.2"),
            ("pr-k.1.1", "pr-k.1.2"),
            ("pr-k.08", "pr-k.8"),
            ("pr-k.000", "pr-k.0"),
            ("pr-k.007", "pr-k.7"),
            ("pr-k.007", "pr-k.9"),
            ("pr-k.000", "pr-k.3"),
            ("pr-j.9", "pr-j.10"),
            ("pr-k.10", "pr-k.11"),
            ("pr-k.1", "pr-k.4"),
        ])
    );
}

#[test]
fn a_copy_counts_past_the_keys_imports_wait_for_and_takes_one_no_longer_waited_for() {
    let waits = |key: &str, named: &[&str]| {
        creating(
            key,
            (named.iter()).map(|named| on(named, None, &[])).collect(),
        )
    };
    let rest = vec![
        at(
            3,
            HERE,
            &[2],
            "create",
            3,
            waits("pr-v", &["pr-k.2", "pr-k.3", "pr-k.5"]),
        ),
        make(4, HERE, 3, "pr-k.1"),
        // past the issue's 1 and the 2 and 3 waited for
        make(5, THERE, 2, "pr-k.1"),
        // 3 is waited for no longer, and the next copy of 1 takes it
        at(6, HERE, &[4], "dep_remove", 3, by_key("pr-k.3", None)),
        make(7, THERE, 5, "pr-k.1"),
        // 2 arrives, an issue's now, and 5 is still waited for
        make(8, THERE, 7, "pr-k.2"),
        make(9, THERE, 8, "pr-k.1"),
        // a new import waits for 7, which a copy counts past, until it is taken back with 5
        at(10, HERE, &[6], "create", 10, waits("pr-w", &["pr-k.7"])),
        make(11, THERE, 9, "pr-k.4"),
        at(12, HERE, &[10], "dep_remove", 10, by_key("pr-k.7", None)),
        at(13, HERE, &[12], "dep_remove", 3, by_key("pr-k.5", None)),
        make(14, THERE, 11, "pr-k.1"),
        make(15, THERE, 14, "pr-k.1"),
        make(16, THERE, 15, "pr-k.1"),
    ];
    let (state, taken) = every_way(&base(), &rest);
    assert!(taken.in_one_batch);
    assert_eq!(
        renames(&state),
        pairs(&[
            ("pr-k.1", "pr-k.4"),
            ("pr-k.1", "pr-k.3"),
            ("pr-k.1", "pr-k.6"),
            ("pr-k.4", "pr-k.8"),
            ("pr-k.1", "pr-k.5"),
            ("pr-k.1", "pr-k.7"),
            ("pr-k.1", "pr-k.9"),
        ])
    );
}

#[test]
fn a_key_two_imports_wait_for_stays_claimed_until_both_are_taken_back() {
    let waits = |key: &str| creating(key, vec![on("pr-k.2", None, &[])]);
    let rest = vec![
        at(3, HERE, &[2], "create", 3, waits("pr-v")),
        at(4, HERE, &[3], "create", 4, waits("pr-w")),
        make(5, HERE, 4, "pr-k.1"),
        make(6, THERE, 2, "pr-k.1"),
        at(7, HERE, &[5], "dep_remove", 3, by_key("pr-k.2", None)),
        make(8, THERE, 6, "pr-k.1"),
        at(
            9,
            HERE,
            &[7],
            "dep_remove",
            4,
            by_key("pr-k.2", Some("blocks")),
        ),
        make(10, THERE, 8, "pr-k.1"),
    ];
    let (state, taken) = every_way(&base(), &rest);
    assert!(taken.in_one_batch);
    assert_eq!(
        renames(&state),
        pairs(&[
            ("pr-k.1", "pr-k.3"),
            ("pr-k.1", "pr-k.4"),
            ("pr-k.1", "pr-k.2"),
        ])
    );
}

#[test]
fn copies_count_on_past_an_i128_and_carry_as_text() {
    // AD01: numbers beyond what bead counts are stored text, and a copy counts on from them, a
    // carry lengthening the number: 9 forty times, with and without a leading zero, and 9 4,299
    // times, the longest number a create may carry, whose copy's key has 4,300 digits.
    let nines = "9".repeat(40);
    let longest = "9".repeat(4299);
    let key = |number: &str| format!("pr-k.{number}");
    let next = |number: &str| key(&format!("{}{number}", &MOST[..MOST.len() - 3]));
    let waited = next("730");
    let rest = vec![
        at(
            3,
            HERE,
            &[2],
            "create",
            3,
            creating("pr-v", vec![on(&waited, None, &[])]),
        ),
        make(4, HERE, 3, &key(BELOW)),
        make(5, HERE, 4, &key(MOST)),
        make(6, HERE, 5, &key(&nines)),
        make(7, HERE, 6, &key(&format!("0{nines}"))),
        make(8, HERE, 7, &key(&longest)),
        make(9, THERE, 2, &key(MOST)),
        make(10, THERE, 9, &key(BELOW)),
        make(11, THERE, 10, &key(BELOW)),
        make(12, THERE, 11, &key(&nines)),
        make(13, THERE, 12, &key(&format!("0{nines}"))),
        make(14, THERE, 13, &key(&longest)),
        make(15, THERE, 14, &key(&longest)),
        // what the import waited for is taken back, and a copy of the largest takes it
        at(16, HERE, &[8], "dep_remove", 3, by_key(&waited, None)),
        make(17, THERE, 15, &key(MOST)),
    ];
    let (state, taken) = every_way(&base(), &rest);
    assert!(taken.in_one_batch);
    let tens = |zeros: usize, last: &str| key(&format!("1{}{last}", "0".repeat(zeros)));
    let expected = [
        (key(MOST), next("728")),
        (key(BELOW), next("729")),
        (key(BELOW), next("731")),
        (key(&nines), tens(40, "")),
        (key(&format!("0{nines}")), tens(39, "1")),
        (key(&longest), tens(4299, "")),
        (key(&longest), tens(4298, "1")),
        (key(MOST), waited),
    ];
    assert_eq!(renames(&state), expected.to_vec());
}

#[test]
fn runs_a_projection_keeps_between_batches_follow_the_imports_taken_back() {
    // One machine's records, each built on the one before it, so that a projection takes them one
    // at a time and, never read back, keeps the runs its copies learned from one to the next.
    let waits = |key: &str, named: &[&str]| {
        creating(
            key,
            (named.iter()).map(|named| on(named, None, &[])).collect(),
        )
    };
    let removal = |n: u128, issue: u128, key: &str| {
        at(n, HERE, &[n - 1], "dep_remove", issue, by_key(key, None))
    };
    let waited = ["pr-k.2", "pr-k.3", "pr-k.4", "pr-k.5", "pr-k.6"];
    let rest = vec![
        at(3, HERE, &[2], "create", 3, waits("pr-v", &waited)),
        make(4, HERE, 3, "pr-k.1"),
        // past the issue's 1 and the 2 to 6 waited for: the run 1 to 6
        make(5, HERE, 4, "pr-k.1"),
        // taken back from the run's end, its middle and then the start of what is left after it,
        // and a copy after each takes the key taken back
        removal(6, 3, "pr-k.6"),
        make(7, HERE, 6, "pr-k.1"),
        removal(8, 3, "pr-k.3"),
        make(9, HERE, 8, "pr-k.1"),
        removal(10, 3, "pr-k.4"),
        make(11, HERE, 10, "pr-k.1"),
        // joining the runs again, past the 5 still waited for and the keys copies took
        make(12, HERE, 11, "pr-k.1"),
        // an import waits for `pr-k.09`, which is not `pr-k.9`
        at(13, HERE, &[12], "create", 13, waits("pr-w", &["pr-k.09"])),
        make(14, HERE, 13, "pr-k.9"),
        make(15, HERE, 14, "pr-k.10"),
        make(16, HERE, 15, "pr-k.8"),
        // `09` is inside the run 1 to 11 as runs order numbers, a longer one after a shorter, but
        // with its leading zero it is no copy's number: taken back, it frees nothing
        removal(17, 13, "pr-k.09"),
        make(18, HERE, 17, "pr-k.8"),
    ];
    let (state, taken) = every_way(&base(), &rest);
    assert_eq!(taken, CHAINED);
    assert_eq!(
        renames(&state),
        pairs(&[
            ("pr-k.1", "pr-k.7"),
            ("pr-k.1", "pr-k.6"),
            ("pr-k.1", "pr-k.3"),
            ("pr-k.1", "pr-k.4"),
            ("pr-k.1", "pr-k.8"),
            ("pr-k.8", "pr-k.11"),
            ("pr-k.8", "pr-k.12"),
        ])
    );
}

mod histories;
