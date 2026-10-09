//! `diff json`'s own tests: a kept case reads back, and `--fixtures` and `--write-cases` are
//! honoured.

use std::path::PathBuf;

use super::*;

fn options(fixtures: Vec<PathBuf>, cases: usize, write_cases: Option<PathBuf>) -> Options {
    Options {
        mode: "json".into(),
        cases,
        seed: 1,
        jobs: 1,
        fixtures,
        write_cases,
    }
}

/// A fresh scratch directory for one test.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("xtask-diff-json-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_kept_case_reads_back_as_itself() {
    let bytes = JsonCase::Bytes((0..=255).collect());
    let digits = JsonCase::PlusOne("9".repeat(4300));
    for case in [bytes, digits] {
        assert_eq!(JsonCase::from_line(&case.to_line()), Ok(case));
    }
    let malformed = [
        "",
        "[]",
        "{}",
        r#"{"bytes": 1}"#,
        r#"{"other": "1"}"#,
        r#"{"bytes": "a", "plus_one": "1"}"#,
        "{\"bytes\": \"\u{101}\"}",
    ];
    for line in malformed {
        assert!(
            JsonCase::from_line(line).is_err(),
            "{line:?} read as a case"
        );
    }
}

#[test]
fn fixtures_replace_the_corpus_and_a_missing_one_is_an_error() {
    let dir = scratch("fixtures");
    let kept = dir.join("json-7.jsonl");
    let case = JsonCase::Bytes(b"[1, 2]".to_vec());
    std::fs::write(&kept, format!("{}\n\n", case.to_line())).unwrap();
    assert_eq!(
        cases(&options(vec![kept], 0, None)).unwrap(),
        (vec![case], 1)
    );
    let corpus = corpus::all().len();
    let (all, first) = cases(&options(Vec::new(), 5, None)).unwrap();
    assert_eq!((all.len(), first), (corpus + 5, corpus));
    assert!(cases(&options(vec![dir.join("missing.jsonl")], 0, None)).is_err());
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The whole command, against CPython: a kept case runs alone, and `--write-cases` records it.
#[test]
fn a_kept_case_reruns_alone_and_is_written() {
    let dir = scratch("rerun");
    let kept = dir.join("json-7.jsonl");
    let line = JsonCase::Bytes(br#"{"a": [1, 2.5, null]}"#.to_vec()).to_line();
    std::fs::write(&kept, format!("{line}\n")).unwrap();
    let written = dir.join("cases.jsonl");
    let options = options(vec![kept], 0, Some(written.clone()));
    assert_eq!(differences(&options).unwrap(), 0);
    assert_eq!(
        std::fs::read_to_string(&written).unwrap(),
        format!("{line}\n")
    );
    std::fs::remove_dir_all(&dir).unwrap();
}
