//! Repository hygiene: tracked files must not expose maintainer home directories, LAN or tailnet
//! hostnames, or SSH targets. Patterns describe shapes rather than private names. ASCII word
//! boundaries deliberately also catch matches beside non-ASCII letters. Lint exemptions sit at a
//! crate or module root, apart from the two `CONTRIBUTING.md` names, no Rust source grows past
//! the line it should have been split long before, nothing is named for a finding, a
//! module's items are imported in one `use` line, the store's and sync's errors of the
//! operating system name their path, no Python catches every exception, and no doc reads a mode
//! as a flag.

/// The repository's tracked files that `pathspecs` match (all of them when none is given), by their
/// paths from its root. A listing without `expected` fails the test: a scan of nothing would pass.
pub(crate) fn tracked(pathspecs: &[&str], expected: &str) -> Vec<String> {
    let listed = std::process::Command::new("git")
        .args(["ls-files", "-z", "--"])
        .args(pathspecs)
        .current_dir(crate::workspace::root())
        .output()
        .expect("git runs");
    assert!(listed.status.success(), "git ls-files failed");
    let names = String::from_utf8(listed.stdout).expect("UTF-8 file names");
    let names: Vec<String> = (names.split('\0'))
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect();
    assert!(
        names.iter().any(|name| name == expected),
        "{expected} not listed"
    );
    names
}

/// Every tracked file `pathspecs` match, its path from the repository's root and its text: what a
/// scan of the repository reads, failing as [`tracked`] does when `expected` is not among them.
pub(crate) fn sources(pathspecs: &[&str], expected: &str) -> Vec<(String, String)> {
    let root = crate::workspace::root();
    (tracked(pathspecs, expected).into_iter())
        .map(|name| {
            let text = std::fs::read_to_string(root.join(&name))
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            (name, text)
        })
        .collect()
}

/// Every tracked Rust source, its path from the repository's root and its text.
fn rust_sources() -> Vec<(String, String)> {
    sources(&["*.rs"], "xtask/src/hygiene.rs")
}

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
        let names = super::tracked(&[], "README.md");
        let found = findings(&workspace::root(), names.iter().map(String::as_str))
            .unwrap_or_else(|error| panic!("{error}"));
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
        let found: Vec<String> = (super::rust_sources().iter())
            .flat_map(|(name, text)| unaccepted(name, text))
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
        let sources = super::rust_sources();
        let found = too_long(
            sources
                .iter()
                .map(|(name, text)| (name.as_str(), text.as_str())),
        );
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

    /// The ids of this tracker's issues that `texts` cite (`roll-k2wy`), as a name would carry
    /// them (`roll_k2wy`): the tracker itself lives outside the repository, so the issues a name
    /// could be named for are those the repository cites.
    fn issue_ids<'a>(texts: impl IntoIterator<Item = &'a str>) -> Vec<String> {
        let cited = Regex::new(r"\broll-([a-z0-9]{4})\b").expect("a valid pattern");
        let mut ids: Vec<String> = (texts.into_iter())
            .flat_map(|text| {
                cited
                    .captures_iter(text)
                    .map(|found| format!("roll_{}", &found[1]))
            })
            .collect();
        ids.sort();
        ids.dedup();
        ids
    }

    /// The functions and modules of `text`, the file `name`, named for one of `issues`: the id as
    /// whole words of the name (`meets_roll_k2wy_18`), as NAME:LINE: ITEM.
    fn named_for_an_issue(name: &str, text: &str, issues: &[String]) -> Vec<String> {
        let item = Regex::new(r"(?m)^[ \t]*(?:pub(?:\([a-z]+\))? )?(?:fn|mod) ([a-z0-9_]+)")
            .expect("a valid pattern");
        let words = |name: &str| format!("_{name}_");
        (item.captures_iter(text))
            .filter(|found| {
                let name = words(&found[1]);
                issues.iter().any(|issue| name.contains(&words(issue)))
            })
            .map(|found| {
                let at = found.get(0).expect("the match").start();
                let line = text[..at].matches('\n').count() + 1;
                format!("{name}:{line}: {}", found[0].trim())
            })
            .collect()
    }

    /// No function or module is named for a finding or an issue: a test is named for the contract
    /// it checks and cites its finding or issue in its doc comment (`AGENTS.md`, *Making a change*).
    #[test]
    fn nothing_is_named_for_a_finding() {
        let root = workspace::root();
        let ledger = std::fs::read_to_string(root.join("docs/FINDINGS.md")).expect("the ledger");
        let ids = finding_ids(&ledger);
        // A ledger read as empty would pass everything: it has hundreds of rows.
        assert!(ids.len() > 100, "{} finding ids", ids.len());
        let sources = super::rust_sources();
        let docs = ["docs/FINDINGS.md", "docs/REVIEWING.md", "CHANGELOG.md"]
            .map(|doc| std::fs::read_to_string(root.join(doc)).expect("a doc"));
        let cited =
            (docs.iter().map(String::as_str)).chain(sources.iter().map(|(_, t)| t.as_str()));
        let issues = issue_ids(cited);
        // Cited ids read as none would pass everything: the docs cite dozens.
        assert!(issues.len() > 3, "{issues:?}");
        let found: Vec<String> = (sources.iter())
            .flat_map(|(name, text)| {
                let mut found = named_for_a_finding(name, text, &ids);
                found.extend(named_for_an_issue(name, text, &issues));
                found
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

    /// A name holding a cited issue's id as whole words is found; `roll` inside a word, as in
    /// `beadroll_core`, or followed by words no cited issue has, as in `roll_back`, is not.
    #[test]
    fn a_cited_issue_id_is_found_only_as_whole_words_of_a_name() {
        let issues = issue_ids([
            "see roll-k2wy.18 and (roll-lqjj)",
            "not roll-toolong or xroll-abcd",
        ]);
        assert_eq!(issues, ["roll_k2wy", "roll_lqjj"]);
        let text = "fn meets_roll_k2wy_18() {}\n  mod roll_lqjj {}\nfn beadroll_core_x() {}\n\
                    fn enroll_lqjj() {}\nfn roll_back() {}\nfn a_roll_lqjjx() {}\n";
        assert_eq!(
            named_for_an_issue("x.rs", text, &issues),
            ["x.rs:1: fn meets_roll_k2wy_18", "x.rs:2: mod roll_lqjj"]
        );
    }
}

/// `ImportLines`.
mod import_lines {
    use std::collections::BTreeMap;

    use regex_lite::Regex;

    /// The modules `text`, the file `name`, imports from in more than one top-level `use` line, as
    /// NAME:LINE: MODULE for each line after the first. A `pub use`, an import under an attribute
    /// and one with nested braces or a glob are left out, as rustfmt does not merge them either.
    fn split(name: &str, text: &str) -> Vec<String> {
        let line = Regex::new(r"(?m)^use ([a-z_][a-z0-9_]*(?:::[a-z_][a-z0-9_]*)*)::(?:\{[^{}]*\}|[A-Za-z_][A-Za-z0-9_]*(?: as [A-Za-z_][A-Za-z0-9_]*)?);")
            .expect("a valid pattern");
        let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
        let mut found = Vec::new();
        for import in line.captures_iter(text) {
            let whole = import.get(0).expect("the match");
            let before = text[..whole.start()].trim_end_matches('\n');
            if before
                .rsplit('\n')
                .next()
                .is_some_and(|line| line.starts_with("#["))
            {
                continue;
            }
            let module = import.get(1).expect("the module").as_str();
            let count = seen.entry(module).or_default();
            *count += 1;
            if *count > 1 {
                let at = text[..whole.start()].matches('\n').count() + 1;
                found.push(format!("{name}:{at}: {module}"));
            }
        }
        found
    }

    /// Every Rust source imports a module's items in one `use` line: a new item joins the braces
    /// there, rather than a line of its own beside them.
    #[test]
    fn a_modules_items_are_imported_in_one_line() {
        let found: Vec<String> = (super::rust_sources().iter())
            .flat_map(|(name, text)| split(name, text))
            .collect();
        assert!(
            found.is_empty(),
            "merge these into the module's other import:\n{}",
            found.join("\n")
        );
    }

    /// A second line for a module is found, braced or not; another module, a `pub use`, an import
    /// under an attribute and one with nested braces are not.
    #[test]
    fn a_second_line_for_one_module_is_found() {
        let text = "use a::b::{C, d};\nuse a::b::E;\nuse a::c::F;\npub use a::b::G;\n\
                    #[cfg(test)]\nuse a::b::H;\nuse a::b::{i::{J}, K};\n";
        assert_eq!(split("x.rs", text), ["x.rs:2: a::b"]);
    }
}

/// `PathlessErrors`.
mod pathless_errors {
    /// The crates whose every operating-system error concerns a file they name: the store's and
    /// sync's. Only git's runner (pipes, child processes) and the CLI (standard input, the
    /// terminal) meet errors about no file in particular.
    const NAMING: [&str; 2] = ["crates/beadroll-store/src/", "crates/beadroll-sync/src/"];

    /// The lines of `text`, the file `name`, that make an error of the operating system without
    /// its path, as NAME:LINE.
    fn pathless(name: &str, text: &str) -> Vec<String> {
        if !NAMING.iter().any(|crate_src| name.starts_with(crate_src)) {
            return Vec::new();
        }
        (text.lines().enumerate())
            .filter(|(_, line)| line.contains("IoError::bare("))
            .map(|(at, _)| format!("{name}:{}", at + 1))
            .collect()
    }

    /// The store and sync name the path of every error of the operating system they report, as
    /// `CONTRIBUTING.md` asks (`{strerror}: {path}`): `IoError::at`, never `IoError::bare`.
    #[test]
    fn the_store_and_sync_name_the_path_of_every_os_error() {
        let found: Vec<String> = (super::rust_sources().iter())
            .flat_map(|(name, text)| pathless(name, text))
            .collect();
        assert!(
            found.is_empty(),
            "name the path with IoError::at:\n{}",
            found.join("\n")
        );
    }

    /// A path-less error is found in the store and sync, and passed over in git and the CLI.
    #[test]
    fn a_pathless_error_is_found_only_where_a_path_is_known() {
        let line = "    file.write_all(data).map_err(|e| IoError::bare(&e))?;\n";
        let text = format!("fn a() {{}}\n{line}");
        assert_eq!(
            pathless("crates/beadroll-store/src/journal.rs", &text),
            ["crates/beadroll-store/src/journal.rs:2"]
        );
        assert!(pathless("crates/beadroll-git/src/runner.rs", &text).is_empty());
        assert!(pathless("crates/beadroll/src/commands.rs", &text).is_empty());
    }
}

/// `CatchAlls`.
mod catch_alls {
    /// The lines of `text`, the Python file `name`, whose `except` catches every exception (`except:`,
    /// `except Exception`, `except BaseException`), as NAME:LINE.
    fn catch_alls(name: &str, text: &str) -> Vec<String> {
        let every = regex_lite::Regex::new(r"^\s*except\s*(?::|\(?\s*(?:Base)?Exception\b)")
            .expect("the pattern compiles");
        (text.lines().enumerate())
            .filter(|(_, line)| every.is_match(line))
            .map(|(at, _)| format!("{name}:{}", at + 1))
            .collect()
    }

    /// Every tracked Python script names the exceptions it expects: one that catches them all
    /// takes a bug of its own for the failure it was written for (a driver logging `CRASH` for its
    /// own mistake) and hides it, and the next script copies it.
    #[test]
    fn python_names_the_exceptions_it_catches() {
        let found: Vec<String> = (super::sources(&["*.py"], "infra/drive.py").iter())
            .flat_map(|(name, text)| catch_alls(name, text))
            .collect();
        assert!(
            found.is_empty(),
            "name the exceptions expected there:\n{}",
            found.join("\n")
        );
    }

    /// A bare `except:`, and `except Exception` or `BaseException` alone, as a tuple's first or
    /// bound to a name, are found; named exceptions, and `Exception` in a string or a comment, are
    /// not.
    #[test]
    fn a_catch_all_is_found_and_a_named_exception_is_not() {
        let text = "try:\n    x()\nexcept:\n    pass\n    except Exception as error:\n\
                    except (Exception, OSError):\nexcept BaseException:\nexcept ExceptionGroup:\n\
                    except (ValueError, KeyError):\nexcept OSError:\n# except Exception\n\
                    print(\"except Exception\")\n";
        assert_eq!(
            catch_alls("x.py", text),
            ["x.py:3", "x.py:5", "x.py:6", "x.py:7"]
        );
    }
}

/// `ModesReadAsFlags`.
mod modes_read_as_flags {
    use regex_lite::Regex;

    /// The parameters of the function a doc block comes before, by name, with their types, when
    /// `signature` (the lines after the block) declares one.
    fn parameters(signature: &str) -> Vec<(String, String)> {
        let function = Regex::new(r"(?s)\bfn\s+\w+(?:<[^>]*>)?\s*\((.*?)\)\s*(?:->|\{|where|;)")
            .expect("a valid pattern");
        let Some(found) = function.captures(signature) else {
            return Vec::new();
        };
        let parameter =
            Regex::new(r"^\s*(?:mut\s+)?(\w+)\s*:\s*(.+?)\s*$").expect("a valid pattern");
        let mut depth = 0usize;
        let mut parts = vec![String::new()];
        for c in found[1].chars() {
            match c {
                '<' | '(' | '[' => depth += 1,
                '>' | ')' | ']' => depth = depth.saturating_sub(1),
                ',' if depth == 0 => {
                    parts.push(String::new());
                    continue;
                }
                _ => {}
            }
            parts.last_mut().expect("a part").push(c);
        }
        (parts.iter())
            .filter_map(|part| parameter.captures(part))
            .map(|named| (named[1].to_owned(), named[2].to_owned()))
            .collect()
    }

    /// The doc lines of `text`, the Rust file `name`, that read a parameter of a mode's type as a
    /// flag (``when `color`.``, ``unless `ok`,``), as NAME:LINE: the doc of a parameter that was a
    /// bool, left as it was when the parameter became a mode. A verb after the name (``when
    /// `colors` says so``) is how a mode is read; `bool` and `Option` parameters are flags.
    fn flags(name: &str, text: &str) -> Vec<String> {
        let read_as_flag =
            Regex::new(r"\b(?:when|unless|if)\s+`(\w+)`\s*(?:[,.;:)]|$|and\b|or\b|else\b)")
                .expect("a valid pattern");
        let lines: Vec<&str> = text.lines().collect();
        let mut found = Vec::new();
        let mut at = 0;
        while at < lines.len() {
            if !lines[at].trim_start().starts_with("///") {
                at += 1;
                continue;
            }
            let start = at;
            while at < lines.len() && lines[at].trim_start().starts_with("///") {
                at += 1;
            }
            let doc = (lines[start..at].iter())
                .map(|line| line.trim_start().trim_start_matches('/').trim())
                .collect::<Vec<_>>()
                .join(" ");
            let after = (lines[at..].iter()).skip_while(|line| line.trim_start().starts_with("#["));
            let signature: String = after.take(12).copied().collect::<Vec<_>>().join("\n");
            let parameters = parameters(&signature);
            for read in read_as_flag.captures_iter(&doc) {
                let mode = (parameters.iter()).find(|(named, _)| *named == read[1]);
                if let Some((_, kind)) = mode
                    && kind != "bool"
                    && !kind.starts_with("Option<")
                {
                    found.push(format!("{name}:{}: `{}`: {kind}", start + 1, &read[1]));
                }
            }
        }
        found
    }

    /// No doc reads a parameter of a mode's type as a flag: when a bool became a mode
    /// (`CONTRIBUTING.md`), its doc says which value it means, or a reader looks for a bool to pass.
    #[test]
    fn no_doc_reads_a_mode_as_a_flag() {
        let found: Vec<String> = (super::rust_sources().iter())
            .flat_map(|(name, text)| flags(name, text))
            .collect();
        assert!(
            found.is_empty(),
            "say which value of the mode is meant:\n{}",
            found.join("\n")
        );
    }

    /// A mode read as a flag is found; a mode with a verb after it, a bool, an `Option`, and a name
    /// that is no parameter are not.
    #[test]
    fn a_mode_read_as_a_flag_is_found_and_a_mode_read_as_one_is_not() {
        let text = "/// Painted when `color`.\nfn render(line: &Line, color: Color) -> String {\n\
                    }\n/// Painted when `color` says so.\nfn paint(color: Color) {}\n\
                    /// Sorted when `sorted`, kept when `kept`.\n\
                    fn sort(sorted: bool, kept: Option<Vec<u8>>) {}\n\
                    /// Fails unless `other`.\nfn check(color: Color) {}\n";
        assert_eq!(flags("x.rs", text), ["x.rs:1: `color`: Color"]);
    }
}
