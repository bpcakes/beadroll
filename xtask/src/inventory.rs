//! `cargo xtask inventory`: every Python test and the Rust tests that port it (`docs/RUST-PORT.md`
//! §4.2).
//!
//! The Python tests come from the suite itself (`xtask/python/oracle.py --list`) and must be
//! exactly those `xtask/inventory.txt` lists; the Rust tests come from `cargo test -- --list`. A
//! Python test `test_x.SomeClass.test_does_y` is ported by any Rust test whose path holds the
//! segments `some_class` and then, last, `does_y`, which is the rule's `x::some_class::does_y`
//! or the same with modules in between; a test listed with `=>` is ported by the Rust tests named
//! there instead.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::Path;
use std::process::{Command, ExitCode, Stdio};

use crate::workspace::{self, Failure};

const CLASSES: [&str; 7] = ["A", "A†", "B", "C", "C-env", "C-api", "D"];

/// One line of `xtask/inventory.txt`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub(crate) class: String,
    pub(crate) id: String,
    /// The Rust tests that replace it, when it is not ported under its own name.
    pub(crate) replaced_by: Vec<String>,
}

pub(crate) fn main(args: Vec<OsString>) -> Result<ExitCode, Failure> {
    let mut require_complete = false;
    for arg in args {
        match arg.to_str() {
            Some("--require-complete") => require_complete = true,
            _ => return Err(Failure::Usage(format!("unknown argument {arg:?}"))),
        }
    }
    let root = workspace::root();
    let path = root.join("xtask").join("inventory.txt");
    let text = std::fs::read_to_string(&path).map_err(|e| Failure::io("cannot read", &path, &e))?;
    let entries = parse(&text).map_err(|e| Failure::Failed(format!("{}: {e}", path.display())))?;
    check_against_suite(&entries, &python_tests(&root)?)?;
    let paths = rule_paths(&entries)?;
    let rust = rust_tests(&root)?;
    let mut ported_by_module: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    let mut unported = 0;
    println!(
        "{:<9} {:<6} {:<100} RUST TEST",
        "STATUS", "CLASS", "PYTHON TEST"
    );
    for (entry, expected) in entries.iter().zip(&paths) {
        let found = porting_tests(entry, &rust);
        let module = entry.id.split('.').next().unwrap_or_default();
        let counts = ported_by_module.entry(module).or_default();
        counts.1 += 1;
        let (status, shown) = if found.is_empty() {
            unported += 1;
            let wanted = if entry.replaced_by.is_empty() {
                expected.clone()
            } else {
                entry.replaced_by.join(", ")
            };
            ("unported", format!("({wanted})"))
        } else {
            counts.0 += 1;
            ("ported", found.join(", "))
        };
        println!("{status:<9} {:<6} {:<100} {shown}", entry.class, entry.id);
    }
    println!();
    for (module, (ported, all)) in &ported_by_module {
        println!("{module:<16} {ported:>3} of {all:>3} ported");
    }
    println!(
        "{:<16} {:>3} of {:>3} ported",
        "all",
        entries.len() - unported,
        entries.len()
    );
    if require_complete && unported > 0 {
        return Err(Failure::Failed(format!(
            "{unported} Python tests are not ported yet"
        )));
    }
    Ok(ExitCode::SUCCESS)
}

/// Reads the inventory: `CLASS TEST-ID [=> RUST-TEST[, RUST-TEST...]]`, `#` to the end of a line
/// a comment.
pub(crate) fn parse(text: &str) -> Result<Vec<Entry>, String> {
    let mut entries = Vec::new();
    let mut seen = BTreeSet::new();
    for (number, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        let (head, replacements) = match line.split_once("=>") {
            Some((head, tail)) => (head, Some(tail)),
            None => (line, None),
        };
        let words: Vec<&str> = head.split_whitespace().collect();
        let &[class, id] = words.as_slice() else {
            return Err(format!("line {}: expected CLASS TEST-ID", number + 1));
        };
        if !CLASSES.contains(&class) {
            return Err(format!("line {}: unknown class {class}", number + 1));
        }
        if id.split('.').count() != 3 {
            return Err(format!(
                "line {}: {id} is not module.Class.method",
                number + 1
            ));
        }
        if !seen.insert(id) {
            return Err(format!("line {}: {id} is listed twice", number + 1));
        }
        let replaced_by: Vec<String> = replacements
            .map(|tail| tail.split(',').map(|t| t.trim().to_owned()).collect())
            .unwrap_or_default();
        if replaced_by
            .iter()
            .any(|t| t.is_empty() || t.contains(char::is_whitespace))
        {
            return Err(format!(
                "line {}: expected => RUST-TEST[, RUST-TEST...]",
                number + 1
            ));
        }
        entries.push(Entry {
            class: class.to_owned(),
            id: id.to_owned(),
            replaced_by,
        });
    }
    Ok(entries)
}

/// Fails unless the inventory lists exactly the suite's tests.
fn check_against_suite(entries: &[Entry], suite: &[String]) -> Result<(), Failure> {
    let listed: BTreeSet<&str> = entries.iter().map(|e| e.id.as_str()).collect();
    let present: BTreeSet<&str> = suite.iter().map(String::as_str).collect();
    let missing: Vec<_> = present.difference(&listed).collect();
    let gone: Vec<_> = listed.difference(&present).collect();
    if missing.is_empty() && gone.is_empty() {
        return Ok(());
    }
    let mut problems = Vec::new();
    for id in missing {
        problems.push(format!("  not in xtask/inventory.txt: {id}"));
    }
    for id in gone {
        problems.push(format!("  not in the Python suite: {id}"));
    }
    Err(Failure::Failed(format!(
        "the inventory and the Python suite disagree:\n{}",
        problems.join("\n")
    )))
}

/// The Python suite's test ids, from every `tests/test_*.py`.
fn python_tests(root: &Path) -> Result<Vec<String>, Failure> {
    let modules = workspace::test_modules(root)?;
    let output = Command::new(workspace::python())
        .arg(root.join("xtask").join("python").join("oracle.py"))
        .arg("--list")
        .args(&modules)
        .current_dir(root)
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| Failure::Failed(format!("cannot run python3: {e}")))?;
    if !output.status.success() {
        return Err(Failure::Failed("listing the Python suite failed".into()));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_owned)
        .collect())
}

/// Every Rust test's path in the workspace, ignored ones included.
fn rust_tests(root: &Path) -> Result<BTreeSet<String>, Failure> {
    let output = Command::new(workspace::cargo())
        .args([
            "test",
            "--workspace",
            "--all-targets",
            "--locked",
            "--quiet",
            "--",
            "--list",
        ])
        .current_dir(root)
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| Failure::Failed(format!("cannot run cargo: {e}")))?;
    if !output.status.success() {
        return Err(Failure::Failed("listing the Rust tests failed".into()));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.strip_suffix(": test"))
        .map(str::to_owned)
        .collect())
}

/// `test_x.SomeClass.test_does_y` → `x::some_class::does_y`, for every entry; fails unless the
/// paths are valid and unique, as the naming rule needs them to be.
fn rule_paths(entries: &[Entry]) -> Result<Vec<String>, Failure> {
    let mut seen = BTreeMap::new();
    let mut paths = Vec::new();
    for entry in entries {
        let path = rule_path(&entry.id)
            .ok_or_else(|| Failure::Failed(format!("{} has no valid Rust path", entry.id)))?;
        let (class, method) = segments(&path);
        if let Some(other) = seen.insert((class.to_owned(), method.to_owned()), &entry.id) {
            return Err(Failure::Failed(format!(
                "{other} and {} share a Rust path",
                entry.id
            )));
        }
        paths.push(path);
    }
    Ok(paths)
}

/// The class and method segments of a rule path.
fn segments(path: &str) -> (&str, &str) {
    let mut parts = path.rsplit("::");
    let method = parts.next().unwrap_or_default();
    (parts.next().unwrap_or_default(), method)
}

/// The rule's Rust path for a Python test id, if every segment is a Rust identifier.
pub(crate) fn rule_path(id: &str) -> Option<String> {
    let mut parts = id.split('.');
    let (module, class, method) = (parts.next()?, parts.next()?, parts.next()?);
    let segments = [
        module.strip_prefix("test_")?.to_owned(),
        snake_case(class),
        method.strip_prefix("test_")?.to_owned(),
    ];
    segments
        .iter()
        .all(|s| is_identifier(s))
        .then(|| segments.join("::"))
}

/// A class name in snake case: `G12_RetriedEndOfClaimIsPublished` becomes
/// `g12_retried_end_of_claim_is_published`, `CompatAuditCLI` `compat_audit_cli`, `Review5CLI`
/// `review5_cli` and `V8_Python312` `v8_python312`.
pub(crate) fn snake_case(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut out = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if c == '_' {
            if !out.ends_with('_') {
                out.push('_');
            }
            continue;
        }
        let previous = i.checked_sub(1).and_then(|p| chars.get(p)).copied();
        let next = chars.get(i + 1).copied();
        let boundary = c.is_uppercase()
            && previous.is_some_and(|p| {
                p.is_lowercase()
                    || p.is_ascii_digit()
                    || (p.is_uppercase() && next.is_some_and(char::is_lowercase))
            });
        if boundary && !out.is_empty() && !out.ends_with('_') {
            out.push('_');
        }
        out.extend(c.to_lowercase());
    }
    out
}

const KEYWORDS: [&str; 50] = [
    "abstract", "as", "async", "await", "become", "box", "break", "const", "continue", "crate",
    "do", "dyn", "else", "enum", "extern", "false", "final", "fn", "for", "gen", "if", "impl",
    "in", "let", "loop", "macro", "match", "mod", "move", "mut", "override", "priv", "pub", "ref",
    "return", "self", "static", "struct", "super", "trait", "true", "try", "type", "typeof",
    "unsafe", "unsized", "use", "virtual", "where", "while",
];

fn is_identifier(segment: &str) -> bool {
    let mut chars = segment.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !KEYWORDS.contains(&segment)
}

/// The Rust tests that port `entry`.
fn porting_tests(entry: &Entry, rust: &BTreeSet<String>) -> Vec<String> {
    if !entry.replaced_by.is_empty() {
        let all = entry.replaced_by.iter().all(|t| rust.contains(t));
        return if all {
            entry.replaced_by.clone()
        } else {
            Vec::new()
        };
    }
    let Some(path) = rule_path(&entry.id) else {
        return Vec::new();
    };
    let (class, method) = segments(&path);
    rust.iter()
        .filter(|t| ports(t, class, method))
        .cloned()
        .collect()
}

/// Whether the Rust test `path` holds `class` and then, as its last segment, `method`.
pub(crate) fn ports(path: &str, class: &str, method: &str) -> bool {
    let segments: Vec<&str> = path.split("::").collect();
    match segments.split_last() {
        Some((last, before)) => *last == method && before.contains(&class),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_names_become_snake_case_as_the_plan_spells_them() {
        for (class, snake) in [
            (
                "G12_RetriedEndOfClaimIsPublished",
                "g12_retried_end_of_claim_is_published",
            ),
            ("CompatAuditCLI", "compat_audit_cli"),
            ("Review5CLI", "review5_cli"),
            ("V8_Python312", "v8_python312"),
            ("BpTest", "bp_test"),
            ("HelpAndPrime", "help_and_prime"),
            (
                "G34_NonUtf8GitAndPsOutputIsNotATraceback",
                "g34_non_utf8_git_and_ps_output_is_not_a_traceback",
            ),
            ("ReplayReview4", "replay_review4"),
            (
                "G02_TrackedConfigCannotAdoptAnotherRepositorysStore",
                "g02_tracked_config_cannot_adopt_another_repositorys_store",
            ),
        ] {
            assert_eq!(snake_case(class), snake, "{class}");
        }
    }

    #[test]
    fn the_rule_path_drops_the_test_prefixes() {
        assert_eq!(
            rule_path("test_hardening.G12_RetriedEndOfClaimIsPublished.test_g12_retried_close")
                .as_deref(),
            Some("hardening::g12_retried_end_of_claim_is_published::g12_retried_close")
        );
        assert_eq!(
            rule_path("test_x.Class.test_type"),
            None,
            "a keyword is no segment"
        );
        assert_eq!(rule_path("test_x.Class.helper"), None);
    }

    #[test]
    fn a_rust_test_ports_a_python_test_when_its_path_holds_class_then_method() {
        for path in [
            "hardening::g12_retried::close",
            "hardening::g12::g12_retried::close",
            "cli::hardening::g12_retried::extra::close",
        ] {
            assert!(ports(path, "g12_retried", "close"), "{path}");
        }
        for path in [
            "hardening::g12_retried::close_more",
            "close::g12_retried",
            "g12_retried::close::x",
        ] {
            assert!(!ports(path, "g12_retried", "close"), "{path}");
        }
    }

    #[test]
    fn the_inventory_reads_classes_ids_and_replacements() {
        let text = "# heading\nA  test_x.C.test_y  # note\nD test_x.C.test_z => a::b, c::d\n\n";
        let entries = parse(text).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].replaced_by, ["a::b", "c::d"]);
        for bad in [
            "Q test_x.C.test_y",
            "A test_x.C",
            "A test_x.C.test_y extra",
            "A test_x.C.test_y =>",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        assert!(
            parse("A test_x.C.test_y\nB test_x.C.test_y").is_err(),
            "listed twice"
        );
    }

    #[test]
    fn listed_replacements_must_all_exist() {
        let rust: BTreeSet<String> = ["a::b".to_owned()].into();
        let entry = |replaced_by: &[&str]| Entry {
            class: "D".into(),
            id: "test_x.C.test_y".into(),
            replaced_by: replaced_by.iter().map(|s| (*s).to_owned()).collect(),
        };
        assert_eq!(porting_tests(&entry(&["a::b"]), &rust), ["a::b"]);
        assert!(porting_tests(&entry(&["a::b", "c::d"]), &rust).is_empty());
    }
}
