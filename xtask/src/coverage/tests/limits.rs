//! A test that runs `bead` under `ulimit` takes `UNPROFILED` (`beadroll_test_support`) for that
//! run: under `-f` the profile an instrumented bead writes at exit fails on the limit and says so on
//! the standard error the test reads, under `-n` it may not open its file. This finds the runs in
//! the tracked Rust files under `crates/*/tests/` (an untracked file is not looked at) and wants a
//! use of `UNPROFILED` in code, not in a comment, for each.
//!
//! A run is a string literal in code, a comment or a char being none, that holds `ulimit ` and
//! `exec ` (`sh -c 'ulimit -f N; exec "$@"'`), whether it is a plain literal, a raw one or a
//! `format!` wrapped on to the next line. The probes that run `dd` under a limit (`exec dd`) are not
//! bead, and the messages that name a `ulimit` have no `exec`.

use super::*;

/// Rust source as code and as its string literals.
struct Lexed {
    /// The source without its comments.
    code: String,
    /// The contents of its string literals, escapes as written.
    literals: Vec<String>,
}

/// `source` without its comments, and its string literals: `//` and `/* */` comments, plain, byte
/// and raw strings, and char literals (so that `'"'` opens nothing).
fn lex(source: &str) -> Lexed {
    let bytes = source.as_bytes();
    let (mut code, mut literals) = (Vec::new(), Vec::new());
    let mut at = 0;
    let after = |from: usize, byte: u8| {
        (bytes.get(from..)).and_then(|rest| rest.iter().position(|b| *b == byte))
    };
    while at < bytes.len() {
        let byte = bytes[at];
        let next = bytes.get(at + 1).copied();
        if byte == b'/' && next == Some(b'/') {
            at += after(at, b'\n').unwrap_or(bytes.len() - at);
        } else if byte == b'/' && next == Some(b'*') {
            let mut depth = 0;
            while at < bytes.len() {
                if bytes[at..].starts_with(b"/*") {
                    depth += 1;
                    at += 2;
                } else if bytes[at..].starts_with(b"*/") {
                    depth -= 1;
                    at += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    at += 1;
                }
            }
        } else if byte == b'r'
            && at
                .checked_sub(1)
                .is_none_or(|before| !is_word(bytes[before]))
            && {
                let hashes = bytes[at + 1..].iter().take_while(|b| **b == b'#').count();
                bytes.get(at + 1 + hashes) == Some(&b'"')
            }
        {
            // a raw string: r"..." or r#"..."#, to the quote and as many hashes as opened it
            let hashes = bytes[at + 1..].iter().take_while(|b| **b == b'#').count();
            let start = at + 2 + hashes;
            let close = [b"\"".to_vec(), vec![b'#'; hashes]].concat();
            let end = (start..bytes.len())
                .find(|i| bytes[*i..].starts_with(&close))
                .unwrap_or(bytes.len());
            literals.push(String::from_utf8_lossy(&bytes[start..end]).into_owned());
            code.extend_from_slice(&bytes[at..(end + close.len()).min(bytes.len())]);
            at = end + close.len();
        } else if byte == b'"' {
            let start = at + 1;
            let mut end = start;
            while end < bytes.len() && bytes[end] != b'"' {
                end += if bytes[end] == b'\\' { 2 } else { 1 };
            }
            let end = end.min(bytes.len());
            literals.push(String::from_utf8_lossy(&bytes[start..end]).into_owned());
            code.extend_from_slice(&bytes[at..(end + 1).min(bytes.len())]);
            at = end + 1;
        } else if byte == b'\'' {
            // a char literal ('x', '\n', '"') or a lifetime ('a): only the first is closed at once
            let len = match (next, bytes.get(at + 2)) {
                (Some(b'\\'), _) => after(at + 3, b'\'').map(|close| close + 4),
                (Some(_), Some(b'\'')) => Some(3),
                _ => None,
            };
            let len = len.unwrap_or(1);
            code.extend_from_slice(&bytes[at..(at + len).min(bytes.len())]);
            at += len;
        } else {
            code.push(byte);
            at += 1;
        }
    }
    Lexed {
        code: String::from_utf8_lossy(&code).into_owned(),
        literals,
    }
}

fn is_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// What is wrong with the file `name`, whose text is `source`: it runs `bead` under `ulimit` more
/// often than it takes `UNPROFILED` in code (a `use` is not a use).
fn problem(name: &str, source: &str) -> Option<String> {
    let lexed = lex(source);
    let runs = (lexed.literals.iter())
        .filter(|text| {
            text.contains("ulimit ") && text.contains("exec ") && !text.contains("exec dd")
        })
        .count();
    let takes = (lexed.code.lines())
        .filter(|line| !line.trim_start().starts_with("use ") && line.contains("UNPROFILED"))
        .count();
    (runs > takes).then(|| {
        format!("{name} runs bead under a ulimit {runs} time(s) and takes UNPROFILED {takes}")
    })
}

/// The tracked Rust files of the repository's tests under `crates/`, with their text.
fn test_sources() -> Vec<(String, String)> {
    let root = workspace::root();
    let listed = Command::new("git")
        .args(["ls-files", "-z", "crates"])
        .current_dir(&root)
        .output()
        .expect("git runs");
    assert!(listed.status.success(), "git ls-files failed");
    let names = String::from_utf8(listed.stdout).expect("UTF-8 file names");
    (names.split('\0'))
        .filter(|name| name.ends_with(".rs") && name.contains("/tests/"))
        .map(|name| {
            (
                name.to_owned(),
                std::fs::read_to_string(root.join(name)).expect(name),
            )
        })
        .collect()
}

#[test]
fn every_test_that_runs_bead_under_a_limit_takes_unprofiled() {
    let sources = test_sources();
    assert!(sources.len() > 100, "{} files listed", sources.len());
    let runs: Vec<&str> = (sources.iter())
        .filter(|(_, text)| {
            lex(text)
                .literals
                .iter()
                .any(|l| l.contains("ulimit ") && l.contains("exec "))
        })
        .map(|(name, _)| name.as_str())
        .collect();
    assert!(
        runs.len() >= 3,
        "the scan found the files that limit bead: {runs:?}"
    );
    let problems: Vec<String> = (sources.iter())
        .filter_map(|(name, text)| problem(name, text))
        .collect();
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}

#[test]
fn a_run_under_a_limit_is_found_however_it_is_written_and_a_use_in_code_is_wanted() {
    let sh = |literal: &str| {
        format!("fn f() {{ Command::new(\"/bin/sh\").args([\"-c\", {literal}]); }}")
    };
    let found = |source: &str| problem("t.rs", source).is_some();
    // a plain literal, a format! wrapped on to the next line, a raw string
    assert!(found(&sh(r#""ulimit -n 20; exec \"$@\"""#)));
    assert!(found(
        "let c = format!(\n    \"ulimit -f {blocks}; exec \\\"$@\\\"\",\n);"
    ));
    assert!(found(&sh(r##"r#"ulimit -f 5; exec "$@""#"##)));
    // after char literals that are a quote, an escaped quote and a newline, and a lifetime
    let run = sh(r#""ulimit -f 1; exec x""#);
    assert!(found(&format!(
        "fn g<'a>(q: &'a str) {{ let _ = ('\"', '\\'', '\\n'); {run} }}"
    )));
    // not runs of bead: a probe that execs dd, a message that names a limit, a comment
    assert!(!found(&sh(r#""ulimit -f 1; exec dd if=/dev/zero""#)));
    assert!(!found(
        r#"fn f() { assert!(ok, "ulimit -n {limit}: {}", out); }"#
    ));
    assert!(!found(
        "// \"ulimit -f 1; exec x\"\n/* \"ulimit -f 1; exec x\" */ fn f() {}"
    ));
    // taken in code: one use for one run; a use line, a doc comment and a second run are not
    let run = sh(r#""ulimit -f 1; exec x""#);
    assert!(!found(&format!(
        "{run}\nfn h() {{ c.envs([UNPROFILED]); }}"
    )));
    assert!(found(&format!(
        "use t::UNPROFILED;\n/// takes UNPROFILED\n// UNPROFILED\n{run}"
    )));
    assert!(found(&format!(
        "{run}\n{run}\nfn h() {{ c.envs([UNPROFILED]); }}"
    )));
}

#[test]
fn stripping_the_use_from_the_real_helper_is_found() {
    let sources = test_sources();
    let (name, text) = sources
        .iter()
        .find(|(name, _)| name.ends_with("hardening/v6.rs"))
        .expect("the V6 helper");
    assert!(problem(name, text).is_none());
    // its doc comment names UNPROFILED, as it should; the code is what counts
    assert!(text.contains("[`UNPROFILED`]"));
    let stripped = text
        .replace(".envs([UNPROFILED])", "")
        .replace("UNPROFILED, ", "");
    assert!(problem(name, &stripped).is_some());
}
