use std::collections::{HashMap, HashSet};

use super::*;
use crate::projection::tests::Rng;

/// The largest number an `i128` holds, without its last digit.
const NEAR_MOST: &str = "17014118346046923173168730371588410572";

/// `runs`' runs, each as its stem, first number and the number after its last.
fn listed(runs: &Runs) -> Vec<(String, String, String)> {
    runs.listed()
}

/// Runs written as [`listed`] gives them.
fn written(runs: &[(&str, &str, &str)]) -> Vec<(String, String, String)> {
    (runs.iter())
        .map(|(stem, start, end)| ((*stem).to_owned(), (*start).to_owned(), (*end).to_owned()))
        .collect()
}

#[test]
fn numbers_order_by_length_then_digits_and_count_on_with_a_carry() {
    let number = |text: &str| Number(text.to_owned());
    assert!(number("9") < number("10"));
    assert!(number("10") < number("11"));
    assert!(number("99") < number("100"));
    assert!(number(&format!("{NEAR_MOST}7")) < number(&format!("{NEAR_MOST}8")));
    assert!(number(&"9".repeat(4299)) < number(&format!("1{}", "0".repeat(4299))));
    assert_eq!(number("0").next(), number("1"));
    assert_eq!(number("9").next(), number("10"));
    assert_eq!(number("1299").next(), number("1300"));
    assert_eq!(
        number(&"9".repeat(4299)).next(),
        number(&format!("1{}", "0".repeat(4299)))
    );
}

#[test]
fn a_run_is_skipped_joined_and_split() {
    let taken: HashSet<&str> = ["s.1", "s.2", "s.4", "s.5", "t.2"].into();
    let claimed = |key: &str| taken.contains(key);
    let mut runs = Runs::default();
    assert_eq!(runs.free("s", "1", claimed), "s.3");
    assert_eq!(listed(&runs), written(&[("s", "1", "3")]));
    // started inside a run, past it, and on to what the next run holds
    assert_eq!(
        runs.free("s", "2", |key| claimed(key) || key == "s.3"),
        "s.6"
    );
    assert_eq!(listed(&runs), written(&[("s", "1", "6")]));
    // another stem counts apart
    assert_eq!(runs.free("t", "2", claimed), "t.3");
    assert_eq!(listed(&runs), written(&[("s", "1", "6"), ("t", "2", "3")]));
    // taken out of the middle, the first and the last of a run
    runs.forget("s.3");
    assert_eq!(
        listed(&runs),
        written(&[("s", "1", "3"), ("s", "4", "6"), ("t", "2", "3")])
    );
    runs.forget("s.4");
    runs.forget("s.2");
    assert_eq!(
        listed(&runs),
        written(&[("s", "1", "2"), ("s", "5", "6"), ("t", "2", "3")])
    );
    runs.forget("t.2");
    assert_eq!(listed(&runs), written(&[("s", "1", "2"), ("s", "5", "6")]));
    // learning the numbers between joins the runs again
    assert_eq!(runs.free("s", "2", |key| key != "s.6"), "s.6");
    assert_eq!(listed(&runs), written(&[("s", "1", "6")]));
    // numbers in no run, keys of no number, and keys with a leading zero change nothing
    for key in [
        "s.6", "s.0", "s", "s.", "s.x", "s.03", "s.003", "u.2", "s.1a",
    ] {
        runs.forget(key);
    }
    assert_eq!(listed(&runs), written(&[("s", "1", "6")]));
}

#[test]
fn a_copy_counts_from_its_number_without_leading_zeros_and_carries_into_a_longer_number() {
    // the keys claimed only grow, as an issue keeps its key: each copy's key joins them
    let nines = "9".repeat(4299);
    let longest = format!("x.{nines}");
    let taken = std::cell::RefCell::new(HashSet::from(["x.7".to_owned(), "x.0".to_owned()]));
    let mut runs = Runs::default();
    let mut copy = |stem: &str, digits: &str| {
        let free = runs.free(stem, digits, |key| taken.borrow().contains(key));
        taken.borrow_mut().insert(free.clone());
        free
    };
    assert_eq!(copy("x", "007"), "x.8");
    assert_eq!(copy("x", "000"), "x.1");
    assert_eq!(copy("y", "00"), "y.0");
    taken.borrow_mut().insert(longest);
    let carried = format!("x.1{}", "0".repeat(4299));
    assert_eq!(copy("x", &nines), carried);
    let end = format!("1{}1", "0".repeat(4298));
    assert_eq!(copy("x", &format!("0{nines}")), format!("x.{end}"));
    assert_eq!(
        listed(&runs),
        written(&[("x", "0", "1"), ("x", "7", "8"), ("x", &nines, &end)])
    );
}

/// A key of the model: under one of three stems, one of them a key under another and one with a dot
/// of its own; mostly a small number, now and then with leading zeros, about the largest an `i128`
/// holds, or nines that carry.
fn key(rng: &mut Rng) -> String {
    let stem = ["s", "s.1", "t"][rng.below(3)];
    let n = rng.below(24);
    let number = match rng.below(30) {
        0 | 1 => format!("0{n}"),
        2 => format!("00{n}"),
        3 | 4 => format!("{NEAR_MOST}{}", rng.below(10)),
        5 => "9".repeat(39 + rng.below(2)),
        6 => format!("1{}", "0".repeat(39 + rng.below(2))),
        _ => n.to_string(),
    };
    format!("{stem}.{number}")
}

/// Fails unless every number of every run is claimed: an issue has its key, or an import waits for
/// it.
fn sound(runs: &Runs, claimed: &dyn Fn(&str) -> bool) {
    for (stem, start, end) in runs.listed() {
        let (mut number, end) = (Number(start), Number(end));
        while number < end {
            let key = format!("{stem}.{}", number.0);
            assert!(claimed(&key), "{key} is in a run and claimed by nothing");
            number = number.next();
        }
    }
}

#[test]
fn every_copy_takes_the_key_counting_one_number_at_a_time_takes() {
    let (mut copies, mut skipped, mut forgotten) = (0, 0, 0);
    for seed in 0..300 {
        let mut rng = Rng(20_261_026 + seed);
        let mut issues: HashSet<String> = HashSet::new();
        // the keys imports wait for, each as often as one does
        let mut waited: HashMap<String, usize> = HashMap::new();
        let mut runs = Runs::default();
        for _ in 0..200 {
            match rng.below(20) {
                0..=8 => {
                    let key = key(&mut rng);
                    let claimed = |key: &str| issues.contains(key) || waited.contains_key(key);
                    if !claimed(&key) {
                        issues.insert(key); // its own key: no copy
                        continue;
                    }
                    let (stem, digits) = key::split_number(&key).unwrap_or_default();
                    let rule = key::free_key(&key, "u", &claimed);
                    let (free, work) = crate::work::counted(|| runs.free(stem, digits, claimed));
                    assert_eq!(Some(&free), rule.as_ref(), "a copy of {key}, seed {seed}");
                    copies += 1;
                    // without a run to skip, a copy looks for one and learns each number it counts
                    // past, one more than how far it counts
                    let number = |key: &str| key::split_number(key)?.1.parse::<usize>().ok();
                    if let (Some(from), Some(to)) = (number(&key), number(&free)) {
                        skipped += usize::from(work.of(Work::Key) < 1 + to - from);
                    }
                    issues.insert(free);
                }
                9..=12 => {
                    issues.insert(key(&mut rng));
                }
                13..=15 => {
                    *waited.entry(key(&mut rng)).or_default() += 1;
                }
                _ => {
                    let mut keys: Vec<&String> = waited.keys().collect();
                    keys.sort();
                    let Some(key) = keys.get(rng.below(keys.len())).map(|key| (*key).clone())
                    else {
                        continue;
                    };
                    let before = runs.listed();
                    if let Some(count) = waited.get_mut(&key) {
                        *count -= 1;
                        if *count == 0 {
                            waited.remove(&key);
                        }
                    }
                    // as replay takes back an import's key: whether anything still claims it or not
                    runs.forget(&key);
                    forgotten += usize::from(runs.listed() != before);
                }
            }
            sound(&runs, &|key| {
                issues.contains(key) || waited.contains_key(key)
            });
        }
    }
    assert!(
        copies > 10_000 && skipped > 1_000 && forgotten > 100,
        "{copies} copies, {skipped} that skipped a run, {forgotten} forgets that split one"
    );
}
