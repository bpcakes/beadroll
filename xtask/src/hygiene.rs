//! `tests/test_hygiene.py`, ported (`docs/RUST-PORT.md` §4.3): the repository carries no trace of
//! any maintainer's environment, no home directories, LAN or tailnet hostnames, or ssh targets. The
//! patterns are Python's, shapes rather than names, so this file leaks nothing itself; they run over
//! every file `git ls-files` names, as there. Their `\b` is ASCII here and Unicode in Python, so
//! these find everything Python's find and, beside a non-ASCII letter, a little more.

/// `RepositoryHygiene`.
mod repository_hygiene {
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
        // A scan of nothing would pass: it must cover this repository.
        assert!(
            names.split('\0').any(|name| name == "README.md"),
            "no README.md listed"
        );
        let shapes: Vec<(&str, Regex)> = (SHAPES.iter())
            .map(|&(what, pattern)| (what, Regex::new(pattern).expect("a valid pattern")))
            .collect();
        let mut found = Vec::new();
        for name in names.split('\0').filter(|name| !name.is_empty()) {
            let path = root.join(name);
            let skipped = [".png", ".jsonl"]
                .iter()
                .any(|suffix| name.ends_with(suffix));
            if !path.is_file() || skipped {
                continue;
            }
            // A file that is not UTF-8 text is not read, as Python's read_text would refuse it.
            let Ok(text) = std::fs::read_to_string(&path) else {
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
        assert!(found.is_empty(), "\n{}", found.join("\n"));
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
