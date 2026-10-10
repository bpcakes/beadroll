//! `V6_UserSuppliedPaths`: an operating system error on a path the user gave is `IO`, exit 1, and
//! writes nothing; `export -o` creates the folders its path needs. As root, `chmod 0` denies
//! nothing, so the class skips there, as the Python one does.

use std::fs::Permissions;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use beadroll_test_support::Machine;
use rustix::fs::{Access, access};

use super::{error_code, id, member, unprivileged};

/// The lines of `text` as Python's `str.splitlines()` splits them: at `\n`, `\r\n`, `\r`, `\v`,
/// `\f`, `\x1c`, `\x1d`, `\x1e`, `\x85`, U+2028 and U+2029, without a last empty line.
fn line_count(text: &str) -> usize {
    let breaks = [
        '\n', '\r', '\x0b', '\x0c', '\x1c', '\x1d', '\x1e', '\u{85}', '\u{2028}', '\u{2029}',
    ];
    let mut count = 0;
    let mut open = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if breaks.contains(&c) {
            if c == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }
            count += 1;
            open = false;
        } else {
            open = true;
        }
    }
    count + usize::from(open)
}

/// `path` as the command line takes it.
fn arg(path: &Path) -> &str {
    path.to_str().expect("a UTF-8 path")
}

/// Verified (V6): an OSError on a user-supplied input path is answered with code IO and exit 1
/// and writes nothing. The review ran 6 kinds (ENOENT, EISDIR, EACCES, ELOOP, ENAMETOOLONG,
/// ENOTDIR) x 5 commands (update --description-file, create --description-file, comments add -f,
/// import, update --claim --description-file): 30/30 IO exit 1, no traceback, the journal event
/// count unchanged and no claim taken.
#[test]
fn oserror_on_a_user_supplied_input_path_is_io_and_writes_nothing() {
    if !unprivileged() {
        return;
    }
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let key = id(a.bead(["create", "x"]));
    a.bead(["sync"]);
    let root = sandbox.root();
    std::fs::write(root.join("afile"), "x").unwrap();
    std::fs::create_dir(root.join("adir")).unwrap();
    let noread = root.join("noread");
    std::fs::write(&noread, "x").unwrap();
    std::fs::set_permissions(&noread, Permissions::from_mode(0o000)).unwrap();
    std::os::unix::fs::symlink("loop", root.join("loop")).unwrap();
    let paths: [PathBuf; 6] = [
        root.join("missing"),
        root.join("adir"),
        noread.clone(),
        root.join("loop"),
        root.join("n".repeat(300)),
        root.join("afile").join("x"),
    ];
    let info = a.bead(["info"]).expect("bead info prints a record");
    let before = member(&info, "events").clone();
    for path in &paths {
        let name = path.file_name().expect("a name").to_string_lossy();
        let label: String = name.chars().take(12).collect();
        let path = arg(path);
        let commands: [&[&str]; 5] = [
            &["update", &key, "--description-file", path],
            &["create", "t", "--description-file", path],
            &["comments", "add", &key, "-f", path],
            &["import", path],
            &["update", &key, "--claim", "--description-file", path],
        ];
        for argv in commands {
            let proc = a.run(argv.iter().chain(&["--json"]));
            assert!(!proc.crashed(), "{label} {}: {}", argv[0], proc.stderr);
            assert_eq!(
                (proc.returncode(), error_code(&proc).as_deref()),
                (1, Some("IO")),
                "{label} {}: {}",
                argv[0],
                proc.stderr
            );
        }
    }
    let info = a.bead(["info"]).expect("bead info prints a record");
    assert_eq!(member(&info, "events"), &before);
    let shown = a.show(&key);
    assert!(shown.get("claim").is_none(), "{shown:?}");
    std::fs::set_permissions(&noread, Permissions::from_mode(0o600)).unwrap();
}

/// Verified (V6): `export -o` into an unusable path is IO, and missing parent directories are
/// created. The review measured EISDIR, EACCES, ENAMETOOLONG and ENOTDIR all IO (ENOTDIR reported
/// as 'File exists'), a path under missing parents exit 0, and `-o /dev/stdout` IO ('Operation not
/// permitted: /dev/.stdout.PID.tmp'), a case the test runs only where the test cannot write `/dev`,
/// as the Python one does.
#[test]
fn export_output_path_errors_are_io_and_missing_parents_are_created() {
    if !unprivileged() {
        return;
    }
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    a.bead(["create", "x"]);
    let root = sandbox.root();
    std::fs::write(root.join("afile"), "x").unwrap();
    std::fs::create_dir(root.join("adir")).unwrap();
    let locked = root.join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, Permissions::from_mode(0o500)).unwrap();
    let mut bad: Vec<(&str, PathBuf)> = vec![
        ("EISDIR", root.join("adir")),
        ("EACCES", locked.join("out.jsonl")),
        ("ENAMETOOLONG", root.join("n".repeat(300))),
        ("ENOTDIR", root.join("afile").join("out.jsonl")),
    ];
    if access("/dev", Access::WRITE_OK).is_err() {
        bad.push(("/dev/stdout", PathBuf::from("/dev/stdout")));
    }
    for (name, path) in &bad {
        let proc = a.run(["export", "-o", arg(path), "--json"]);
        assert!(!proc.crashed(), "{name}: {}", proc.stderr);
        assert_eq!(
            (proc.returncode(), error_code(&proc).as_deref()),
            (1, Some("IO")),
            "{name}: {}",
            proc.stderr
        );
    }
    assert_eq!(std::fs::read_to_string(root.join("afile")).unwrap(), "x");
    let left: Vec<PathBuf> = std::fs::read_dir(root.join("adir"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(left, Vec::<PathBuf>::new());
    let target = root.join("new").join("deeper").join("out.jsonl");
    let proc = a.run(["export", "-o", arg(&target), "--json"]);
    assert_eq!(proc.returncode(), 0, "{}", proc.stderr);
    assert_eq!(line_count(&std::fs::read_to_string(&target).unwrap()), 1);
    std::fs::set_permissions(&locked, Permissions::from_mode(0o700)).unwrap();
}
