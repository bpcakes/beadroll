//! The memo as it is written and read back: whole or not at all, and trusted for a file only once
//! the file was settled when the memo was written.

use super::*;

fn key(len: u64) -> FileKey {
    FileKey {
        len,
        modified: 1_791_558_245_123_456_789,
        changed: -5,
        inode: 42,
        device: u64::MAX,
    }
}

fn remembered(len: u64, lines: Option<usize>, blob: Option<&str>) -> Remembered {
    Remembered {
        key: key(len),
        lines,
        blob: blob.map(str::to_owned),
    }
}

#[test]
fn a_memo_reads_back_as_it_was_written() {
    let mut memo = Memo::default();
    let sha1 = "0123456789abcdef0123456789abcdef01234567";
    let sha256 = "ab".repeat(32);
    memo.set("events/m/000001.jsonl", key(10), Some(3), Some(sha1.into()));
    memo.set("events/m/claims/x y.json", key(0), None, Some(sha256));
    memo.set("events/odd\nname/000002.jsonl", key(7), Some(1), None);
    let written = memo.render();
    assert!(written.starts_with(HEADER));
    assert_eq!(parse(&written, &|_| true), Some(memo.files.clone()));
    assert_eq!(parse(HEADER, &|_| true), Some(BTreeMap::new()));
}

#[test]
fn a_memo_that_is_not_whole_is_none() {
    let mut memo = Memo::default();
    let sha = "0123456789abcdef0123456789abcdef01234567";
    memo.set("events/m/000001.jsonl", key(10), Some(3), Some(sha.into()));
    let written = memo.render();
    for cut in 0..written.len() {
        // a record cut short anywhere is no memo, never part of one
        if cut != HEADER.len() {
            assert_eq!(parse(&written[..cut], &|_| true), None, "{cut}");
        }
    }
    let text = String::from_utf8(written.clone()).unwrap();
    for wrong in [
        text.replace("bead journal memo 1", "bead journal memo 2"),
        text.replace(" 3 ", " +3 "),
        text.replace(" 3 ", " three "),
        text.replace(sha, &sha.to_uppercase()),
        text.replace(sha, &sha[..39]),
        text.replace(" 42 ", " 42  "),
        text.replace("events/m/000001.jsonl", ""),
        format!("{text}{}", &text[HEADER.len()..]),
    ] {
        assert_eq!(parse(wrong.as_bytes(), &|_| true), None, "{wrong:?}");
    }
    let mut invalid = written;
    invalid.insert(HEADER.len(), 0xff);
    assert_eq!(parse(&invalid, &|_| true), None);
}

/// A memo read for some of its files keeps only theirs, and is none when any other line is
/// not as a memo is written, as the whole memo would be: what it says of one file never
/// depends on which files were asked for. Its files are in the order `render` writes them,
/// each once.
#[test]
fn a_memo_read_for_some_files_is_checked_whole() {
    let mut memo = Memo::default();
    let sha = "0123456789abcdef0123456789abcdef01234567";
    memo.set("events/m/000001.jsonl", key(10), Some(3), None);
    memo.set("events/m/claims/a.json", key(4), Some(1), Some(sha.into()));
    memo.set("events/m/claims/b.json", key(5), Some(1), Some(sha.into()));
    let written = memo.render();
    let segments = |path: &str| !path.contains("/claims/");
    let only = parse(&written, &segments).unwrap();
    assert_eq!(only.keys().collect::<Vec<_>>(), ["events/m/000001.jsonl"]);
    let text = String::from_utf8(written).unwrap();
    let damaged = text.replace(&format!("1 {sha}\n"), &format!("one {sha}\n"));
    assert_eq!(parse(damaged.as_bytes(), &segments), None);
    // out of order: not as one is written
    let mut lines: Vec<&str> = text[HEADER.len()..].split_inclusive('\n').collect();
    lines.swap(1, 2);
    let swapped = format!("{}{}", "bead journal memo 1\n", lines.concat());
    assert_eq!(parse(swapped.as_bytes(), &|_| true), None);
    assert_eq!(parse(swapped.as_bytes(), &segments), None);
}

#[test]
fn a_memo_says_nothing_of_a_file_it_was_not_written_after() {
    let sha = "0123456789abcdef0123456789abcdef01234567";
    let kept = |written| {
        let mut files = BTreeMap::new();
        files.insert("p".to_owned(), remembered(10, Some(2), Some(sha)));
        settled(&mut files, written);
        files.len()
    };
    let moment = key(10).modified;
    assert_eq!(kept(Some(moment + 1)), 1, "settled before the memo");
    assert_eq!(kept(Some(moment)), 0, "of the memo's own tick");
    assert_eq!(kept(Some(moment - 1)), 0, "written before the file");
    assert_eq!(kept(None), 0, "no time for the memo itself");
    // both times must be older: a change time of that moment is as racy as a write time
    let changed_since = FileKey {
        changed: moment + 1,
        ..key(10)
    };
    assert!(key(10).settled_before(moment + 1));
    assert!(!changed_since.settled_before(moment + 1));
}

#[test]
fn a_blob_is_remembered_only_for_the_identity_it_was_hashed_at() {
    let mut memo = Memo::default();
    let sha = "0123456789abcdef0123456789abcdef01234567";
    memo.files
        .insert("p".into(), remembered(10, Some(2), Some(sha)));
    assert_eq!(memo.blob("p", &key(10)), Some(sha));
    assert_eq!(memo.blob("p", &key(11)), None);
    assert_eq!(memo.blob("q", &key(10)), None);
    // hashed again at another identity: its lines are not known for that one
    memo.hashed("p", key(11), sha);
    assert_eq!(memo.files["p"], remembered(11, None, Some(sha)));
    memo.hashed("p", key(11), sha);
    memo.changed = false;
    memo.hashed("p", key(11), sha);
    assert!(!memo.changed, "nothing new was learnt");
    memo.files
        .insert("q".into(), remembered(3, Some(1), Some(sha)));
    memo.changed = false;
    memo.forget_blobs();
    assert!(memo.changed, "a blob was forgotten");
    assert_eq!(
        memo.files["q"],
        remembered(3, Some(1), None),
        "its lines are kept"
    );
    assert_eq!(memo.blob("q", &key(3)), None);
    memo.retain(|path| path != "p" && path != "q");
    assert!(memo.changed && memo.files.is_empty());
}

/// The lines a memo read for some of its files only checks (roll-lqjj.47) are judged as the lines
/// it makes into values are: whatever a field of one is, `fields_are_written` says what
/// `fields_of` does, at the edges of each number's type and where a number is not one.
#[test]
fn a_line_is_checked_as_it_is_made() {
    let sha1 = "0123456789abcdef0123456789abcdef01234567";
    let sha256 = "ab".repeat(32);
    let u64_max = u64::MAX.to_string();
    let u64_over = "18446744073709551616";
    let i128_max = i128::MAX.to_string();
    let i128_over = "170141183460469231731687303715884105728";
    let i128_min = i128::MIN.to_string();
    let wide = "9".repeat(60);
    // the numbers a field takes in turn, and the blobs that follow the fields
    let numbers = [
        "0",
        "7",
        "-5",
        "-0",
        "+3",
        "",
        "-",
        "3 ",
        " 3",
        "1e3",
        "0x1",
        "٣",
        "١٢",
        "003",
        &u64_max,
        u64_over,
        &i128_max,
        i128_over,
        &i128_min,
        &wide,
        "999999999",
        "1000000000",
        "9999999999999999999",
        "10000000000000000000",
    ];
    let blobs = [
        "-",
        sha1,
        &sha256,
        "",
        "--",
        &sha1[..39],
        &sha1.to_uppercase(),
        &format!("{sha1}0"),
        "g123456789abcdef0123456789abcdef01234567",
    ];
    let mut checked = 0;
    for number in numbers {
        for blob in blobs {
            // each position takes the number in turn, the others being as written
            for at in 0..6 {
                let mut fields = ["10", "1791558245123456789", "-5", "42", "1", "3"];
                fields[at] = number;
                let line = format!("{} {blob}", fields.join(" "));
                assert_eq!(
                    fields_are_written(&line),
                    fields_of(&line).is_some(),
                    "{line:?}"
                );
                checked += 1;
            }
        }
    }
    // the lines of a missing field, an extra one, and none at all
    for line in [
        "",
        " ",
        "10 1 2 3 4 5",
        "10 1 2 3 4 5 - -",
        "10 1 2 3 4 - ",
        "10  1 2 3 4 5 -",
        "10 1 2 3 4 - -",
    ] {
        assert_eq!(
            fields_are_written(line),
            fields_of(line).is_some(),
            "{line:?}"
        );
        checked += 1;
    }
    assert!(checked > 1_000);
    // and the line as written is both
    let written = format!("10 1791558245123456789 -5 42 {} 3 {sha1}", u64::MAX);
    assert!(fields_are_written(&written) && fields_of(&written).is_some());
}

/// A memo read for some of its files has those files' entries as a read of the whole has them,
/// whichever they are (roll-lqjj.47: the others are checked, not made).
#[test]
fn a_memo_read_for_some_files_holds_what_a_read_of_the_whole_holds_of_them() {
    let sha = "0123456789abcdef0123456789abcdef01234567";
    let mut memo = Memo::default();
    for n in 0..20u64 {
        let blob = (n % 3 != 0).then(|| sha.to_owned());
        let lines = (n % 4 != 0).then_some(n as usize);
        memo.set(
            &format!("events/m/{n:06}.jsonl"),
            key(n),
            lines,
            blob.clone(),
        );
        memo.set(
            &format!("events/m/claims/{n:03}.json"),
            key(n + 100),
            Some(1),
            blob,
        );
    }
    let written = memo.render();
    let whole = parse(&written, &|_| true).expect("a memo");
    assert_eq!(whole.len(), 40);
    for wanted in [
        (|path: &str| path.ends_with(".jsonl")) as fn(&str) -> bool,
        |path| path.ends_with(".json"),
        |path| path.contains("00"),
        |_| false,
    ] {
        let part = parse(&written, &wanted).expect("a memo");
        let expected: BTreeMap<_, _> = (whole.iter())
            .filter(|(path, _)| wanted(path))
            .map(|(path, file)| (path.clone(), file.clone()))
            .collect();
        assert_eq!(part, expected);
    }
}
