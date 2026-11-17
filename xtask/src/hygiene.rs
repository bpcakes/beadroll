//! Repository hygiene: tracked files must not expose maintainer home directories, LAN or tailnet
//! hostnames, or SSH targets. Patterns describe shapes rather than private names. ASCII word
//! boundaries deliberately also catch matches beside non-ASCII letters. Lint exemptions sit at a
//! crate or module root, apart from the two `CONTRIBUTING.md` names, no Rust source grows past
//! the line it should have been split long before, and nothing is named for a finding.

/// `RepositoryHygiene`.
mod repository_hygiene {
    use std::fs::Permissions;
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::Path;

    use regex_lite::Regex;

    use crate::workspace;

    /// What each pattern finds, and the pattern.
    const SHAPES: [(&str, &str); 3] = [
        (
            "a home directory",
            r"/(?:Users|home)/[A-Za-z][A-Za-z0-9._-]*/",
        ),
        ("a .local hostname", r"\b[a-z0-9][a-z0-9-]*\.local\b"),
        // a login at a numbered host
        (
            "an ssh target",
            r"\b[a-z][a-z0-9_-]*@[a-z0-9][a-z0-9-]*(?:-[0-9]+)\b",
        ),
    ];

    /// The placeholders the review reports use.
    const ALLOWED: [&str; 4] = [
        "/Users/<name>/",
        "/home/<name>/",
        "/home/hostname/",
        "<hostname>.local",
    ];

    /// Findings from SHAPES in named files under root, excluding placeholders, as
    /// NAME:LINE:WHAT:TEXT. Skip images, JSONL logs and non-UTF-8 files; unreadable files are
    /// errors.
    fn findings<'n>(
        root: &Path,
        names: impl IntoIterator<Item = &'n str>,
    ) -> Result<Vec<String>, String> {
        let shapes: Vec<(&str, Regex)> = (SHAPES.iter())
            .map(|&(what, pattern)| (what, Regex::new(pattern).expect("a valid pattern")))
            .collect();
        let mut found = Vec::new();
        for name in names {
            let path = root.join(name);
            let skipped = [".png", ".jsonl"]
                .iter()
                .any(|suffix| name.ends_with(suffix));
            if !path.is_file() || skipped {
                continue;
            }
            let bytes = std::fs::read(&path).map_err(|error| format!("{name}: {error}"))?;
            let Ok(text) = String::from_utf8(bytes) else {
                continue;
            };
            for (what, shape) in &shapes {
                for shown in shape.find_iter(&text) {
                    if ALLOWED.contains(&shown.as_str()) {
                        continue;
                    }
                    let line = text[..shown.start()].matches('\n').count() + 1;
                    found.push(format!("{name}:{line}: {what}: {}", shown.as_str()));
                }
            }
        }
        Ok(found)
    }

    #[test]
    fn tracked_files_name_no_machine_or_user() {
        let root = workspace::root();
        let listed = std::process::Command::new("git")
            .args(["ls-files", "-z"])
            .current_dir(&root)
            .output()
            .expect("git runs");
        assert!(listed.status.success(), "git ls-files failed");
        let names = String::from_utf8(listed.stdout).expect("UTF-8 file names");
        let names: Vec<&str> = names.split('\0').filter(|name| !name.is_empty()).collect();
        // A scan of nothing would pass: it must cover this repository.
        assert!(names.contains(&"README.md"), "no README.md listed");
        let found = findings(&root, names).unwrap_or_else(|error| panic!("{error}"));
        assert!(found.is_empty(), "\n{}", found.join("\n"));
    }

    /// A file that is not UTF-8 is passed over, whatever it holds, but one the scan cannot read
    /// fails it: it was not checked.
    #[test]
    fn only_text_that_is_not_utf8_is_passed_over() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let home = format!("/{}/{}/", "Users", "someone");
        let latin1 = [b"caf\xe9 ".as_slice(), home.as_bytes()].concat();
        std::fs::write(dir.path().join("latin1.txt"), latin1).expect("a Latin-1 file");
        let skipped = findings(dir.path(), ["latin1.txt"]);
        assert_eq!(skipped, Ok(Vec::new()));
        let locked = dir.path().join("locked.txt");
        std::fs::write(&locked, &home).expect("a text file");
        std::fs::set_permissions(&locked, Permissions::from_mode(0o000)).expect("its mode");
        if std::fs::read(&locked).is_ok() {
            return; // root reads it all the same: no unreadable file can be made here
        }
        let failed = findings(dir.path(), ["locked.txt"]);
        let error = failed.expect_err("an unreadable file fails the scan");
        assert!(error.starts_with("locked.txt: "), "{error}");
    }

    /// Each pattern finds its shape and passes over its near misses. The examples are put together
    /// at run time, so that this file names no shape itself.
    #[test]
    fn each_shape_finds_what_it_names() {
        let finds = |what: &str, text: &str| {
            let (_, pattern) = SHAPES
                .iter()
                .find(|(name, _)| *name == what)
                .expect("a shape");
            let shape = Regex::new(pattern).expect("a valid pattern");
            shape
                .find_iter(text)
                .map(|m| m.as_str().to_owned())
                .collect::<Vec<_>>()
        };
        let home = format!("cd /{}/{}/src", "Users", "someone");
        assert_eq!(
            finds("a home directory", &home),
            [format!("/{}/{}/", "Users", "someone")]
        );
        let host = format!("ping {}.{} now", "box-2", "local");
        assert_eq!(
            finds("a .local hostname", &host),
            [format!("{}.{}", "box-2", "local")]
        );
        let ssh = format!("ssh {}@{}-{}-x", "dev", "linux", "3");
        assert_eq!(
            finds("an ssh target", &ssh),
            [format!("{}@{}-{}", "dev", "linux", "3")]
        );
        assert!(finds("an ssh target", "bead@localhost and git@github.com").is_empty());
        assert!(finds("a .local hostname", "Box.locally").is_empty());
    }
}

/// `LintExemptions`.
mod lint_exemptions {
    use regex_lite::Regex;

    use crate::workspace;

    /// An exemption on one item rather than a whole crate or module: an outer `allow` or `expect`.
    const OUTER: &str = r"(?m)^[ \t]*#\[(?:allow|expect)\(";

    /// The item-level exemptions `CONTRIBUTING.md` accepts, by file and the lint they name:
    /// beadroll-git's `detach`, one of the audited unsafe spots in a module that is otherwise safe,
    /// and the sweep's `sweep::sweep`, kept for the test filters and findings that name it.
    const ACCEPTED: [(&str, &str); 2] = [
        ("crates/beadroll-git/src/runner.rs", "unsafe_code"),
        (
            "crates/beadroll/tests/cli/sweep.rs",
            "clippy::module_inception",
        ),
    ];

    /// The item-level exemptions in `text`, the file `name`, that are not accepted, as NAME:LINE.
    fn unaccepted(name: &str, text: &str) -> Vec<String> {
        let outer = Regex::new(OUTER).expect("a valid pattern");
        outer
            .find_iter(text)
            .filter(|found| {
                let after = &text[found.end()..];
                let lint = after
                    .trim_start()
                    .split([',', ')'])
                    .next()
                    .unwrap_or_default()
                    .trim();
                !ACCEPTED.contains(&(name, lint))
            })
            .map(|found| {
                let line = text[..found.start()].matches('\n').count() + 1;
                format!("{name}:{line}")
            })
            .collect()
    }

    /// Every lint exemption in the repository's Rust is a crate's or module's (`#![allow]`), apart
    /// from the accepted ones: a long test, say, is split rather than exempted (`CONTRIBUTING.md`).
    #[test]
    fn exemptions_sit_at_a_crate_or_module_root() {
        let root = workspace::root();
        let listed = std::process::Command::new("git")
            .args(["ls-files", "-z", "*.rs"])
            .current_dir(&root)
            .output()
            .expect("git runs");
        assert!(listed.status.success(), "git ls-files failed");
        let names = String::from_utf8(listed.stdout).expect("UTF-8 file names");
        let names: Vec<&str> = names.split('\0').filter(|name| !name.is_empty()).collect();
        // A scan of nothing would pass: it must cover this repository.
        assert!(names.contains(&"xtask/src/hygiene.rs"), "no Rust listed");
        let found: Vec<String> = (names.iter())
            .flat_map(|name| {
                let text = std::fs::read_to_string(root.join(name))
                    .unwrap_or_else(|error| panic!("{name}: {error}"));
                unaccepted(name, &text)
            })
            .collect();
        assert!(
            found.is_empty(),
            "item-level exemptions:\n{}",
            found.join("\n")
        );
    }

    /// An outer exemption is found wherever it is indented, unless it is an accepted one; an inner
    /// one, at a root, is not an item's. The examples are put together at run time, so that this
    /// file holds no exemption itself.
    #[test]
    fn an_item_level_exemption_is_found_unless_accepted() {
        let attribute = |inner: &str, lint: &str| format!("{}{inner}[allow(\n    {lint},\n", '#');
        let item = format!(
            "fn a() {{}}\n    {}",
            attribute("", "clippy::too_many_lines")
        );
        assert_eq!(unaccepted("x.rs", &item), ["x.rs:2"]);
        assert!(unaccepted("x.rs", &attribute("!", "clippy::too_many_lines")).is_empty());
        let (name, lint) = ACCEPTED[0];
        assert!(unaccepted(name, &attribute("", lint)).is_empty());
        assert_eq!(
            unaccepted(name, &attribute("", "dead_code")),
            [format!("{name}:1")]
        );
    }
}

/// `FileLengths`.
mod file_lengths {
    use crate::workspace;

    /// The most lines a Rust source may have: `CONTRIBUTING.md` asks for under 500, and a file past
    /// this was not split when it should have been.
    const LIMIT: usize = 800;

    /// The files of `sources`, each its name and text, longer than [`LIMIT`] lines, as NAME: LINES.
    fn too_long<'s>(sources: impl IntoIterator<Item = (&'s str, &'s str)>) -> Vec<String> {
        sources
            .into_iter()
            .map(|(name, text)| (name, text.lines().count()))
            .filter(|&(_, lines)| lines > LIMIT)
            .map(|(name, lines)| format!("{name}: {lines} lines"))
            .collect()
    }

    /// No tracked Rust source is longer than [`LIMIT`]: the next test of a subject goes into a module
    /// of its own, not onto the end of the longest file.
    #[test]
    fn no_source_is_past_the_line_to_split_it() {
        let root = workspace::root();
        let listed = std::process::Command::new("git")
            .args(["ls-files", "-z", "*.rs"])
            .current_dir(&root)
            .output()
            .expect("git runs");
        assert!(listed.status.success(), "git ls-files failed");
        let names = String::from_utf8(listed.stdout).expect("UTF-8 file names");
        let names: Vec<&str> = names.split('\0').filter(|name| !name.is_empty()).collect();
        // A scan of nothing would pass: it must cover this repository.
        assert!(names.contains(&"xtask/src/hygiene.rs"), "no Rust listed");
        let texts: Vec<(&str, String)> = (names.iter())
            .map(|name| {
                let text = std::fs::read_to_string(root.join(name))
                    .unwrap_or_else(|error| panic!("{name}: {error}"));
                (*name, text)
            })
            .collect();
        let found = too_long(texts.iter().map(|(name, text)| (*name, text.as_str())));
        assert!(
            found.is_empty(),
            "split these by subject:\n{}",
            found.join("\n")
        );
    }

    /// A file of exactly [`LIMIT`] lines passes, with or without a last newline; one more fails.
    #[test]
    fn the_limit_counts_lines() {
        let at = "x\n".repeat(LIMIT);
        let over = format!("{at}y");
        assert!(too_long([("a.rs", at.as_str()), ("b.rs", at.trim_end())]).is_empty());
        assert_eq!(
            too_long([("c.rs", over.as_str())]),
            [format!("c.rs: {} lines", LIMIT + 1)]
        );
    }
}

/// `FindingNames`.
mod finding_names {
    use regex_lite::Regex;

    use crate::workspace;

    /// The ids of the findings `ledger` (`docs/FINDINGS.md`) has a row for, lower-cased as a name
    /// would carry them.
    fn finding_ids(ledger: &str) -> Vec<String> {
        let row = Regex::new(r"(?m)^\| ([A-Z]{1,2}[0-9]{2}) \|").expect("a valid pattern");
        (row.captures_iter(ledger))
            .map(|found| found[1].to_lowercase())
            .collect()
    }

    /// The functions and modules of `text`, the file `name`, whose names begin with one of `ids`,
    /// as NAME:LINE: ITEM.
    fn named_for_a_finding(name: &str, text: &str, ids: &[String]) -> Vec<String> {
        let item = Regex::new(
            r"(?m)^[ \t]*(?:pub(?:\([a-z]+\))? )?(?:fn|mod) ([a-z]{1,2}[0-9]{2})_[a-z0-9_]*",
        )
        .expect("a valid pattern");
        (item.captures_iter(text))
            .filter(|found| ids.iter().any(|id| *id == found[1]))
            .map(|found| {
                let at = found.get(0).expect("the match").start();
                let line = text[..at].matches('\n').count() + 1;
                format!("{name}:{line}: {}", found[0].trim())
            })
            .collect()
    }

    /// No function or module is named for a finding: a test is named for the contract it checks
    /// and cites its finding in its doc comment (`AGENTS.md`, *Making a change*).
    #[test]
    fn nothing_is_named_for_a_finding() {
        let root = workspace::root();
        let ledger = std::fs::read_to_string(root.join("docs/FINDINGS.md")).expect("the ledger");
        let ids = finding_ids(&ledger);
        // A ledger read as empty would pass everything: it has hundreds of rows.
        assert!(ids.len() > 100, "{} finding ids", ids.len());
        let listed = std::process::Command::new("git")
            .args(["ls-files", "-z", "*.rs"])
            .current_dir(&root)
            .output()
            .expect("git runs");
        assert!(listed.status.success(), "git ls-files failed");
        let names = String::from_utf8(listed.stdout).expect("UTF-8 file names");
        let found: Vec<String> = (names.split('\0'))
            .filter(|name| !name.is_empty())
            .flat_map(|name| {
                let text = std::fs::read_to_string(root.join(name))
                    .unwrap_or_else(|error| panic!("{name}: {error}"));
                named_for_a_finding(name, &text, &ids)
            })
            .collect();
        assert!(
            found.is_empty(),
            "name these for what they check:\n{}",
            found.join("\n")
        );
    }

    /// A name beginning with a ledger's id is found, as a function or a module and at any
    /// indentation; one beginning with letters and digits that are no finding's is not.
    #[test]
    fn a_finding_prefix_is_found_only_for_a_finding_in_the_ledger() {
        let ids = finding_ids("| G25 | Low | a row |\n| AD01 | High | another |\nprose | X99 |\n");
        assert_eq!(ids, ["g25", "ad01"]);
        let text = "fn ad01_a() {}\n    mod g25_b {}\nfn b36_c() {}\npub(crate) fn g25_d() {}\n";
        assert_eq!(
            named_for_a_finding("x.rs", text, &ids),
            [
                "x.rs:1: fn ad01_a",
                "x.rs:2: mod g25_b",
                "x.rs:4: pub(crate) fn g25_d"
            ]
        );
    }
}
