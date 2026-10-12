//! Storage damage tests directly edit journal files: invalid events, missing final newlines, torn
//! tails, rewritten remote copies and copied machine identities that diverge across rotation.
//! Commands run through the selected bead binary.

use std::ffi::OsStr;
use std::fs::{File, FileTimes, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use beadroll_core::json::{Format, write};
use beadroll_test_support::{Json, Machine, Sandbox, code, git, json, parse_json};

use super::{member, segments, set_up, two};

/// The class's `segment`: the last segment of the machine's own journal.
fn segment(machine: &Machine) -> PathBuf {
    let last = segments(machine).pop();
    last.expect("a segment of the machine's own journal")
}

/// The path of `file` in `machine`'s store: `file.relative_to(machine.store())`.
fn relative(file: &Path, machine: &Machine) -> PathBuf {
    let store = machine.store();
    let relative = file.strip_prefix(&store).expect("a file in the store");
    relative.to_path_buf()
}

/// The folder `path` is in: `path.parent`.
fn parent(path: &Path) -> &Path {
    path.parent().expect("a folder")
}

/// The bytes of the file at `path`: `path.read_bytes()`.
fn bytes(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// `bytes` appended to the file at `path`, as `path.open("ab")` appends them.
fn append(path: &Path, bytes: &[u8]) {
    let file = OpenOptions::new().append(true).create(true).open(path);
    let mut file = file.unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    file.write_all(bytes).expect("the appended bytes");
}

/// The entries of `folder` whose names hold `infix`, as `folder.glob(f"*{infix}*")` lists them:
/// the infix `.torn-` for `folder.glob("*.torn-*")`.
fn named(folder: &Path, infix: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let holds = |path: &Path| {
        let name = path.file_name().expect("a name").to_string_lossy();
        name.contains(infix)
    };
    let paths = entries.map(|entry| entry.expect("a directory entry").path());
    paths.filter(|path| holds(path)).collect()
}

/// Spaced, ASCII-escaped JSON.
fn dumps(value: &Json) -> String {
    write(value, Format::ASCII).expect("a value Python can write")
}

/// The mutable member `key` of object `value`.
fn member_mut<'v>(value: &'v mut Json, key: &str) -> &'v mut Json {
    match value {
        Json::Object(members) => {
            let found = members.get_mut(key);
            found.unwrap_or_else(|| panic!("no {key:?} in the object"))
        }
        other => panic!("{other:?} is not an object"),
    }
}

/// `event["data"]["fields"]["title"] = title`.
fn retitle(event: &mut Json, title: &str) {
    match member_mut(member_mut(event, "data"), "fields") {
        Json::Object(fields) => {
            fields.insert("title".to_owned(), Json::from(title));
        }
        other => panic!("{other:?} is not an object"),
    }
}

/// `shutil.copytree(source, target)`: `target`, which must not exist yet, made a copy of the tree
/// under `source`, symlinks followed. Each file is copied as `copy2` copies it, with its mode and
/// times, and each directory takes its source's times and mode once its contents are in.
fn copytree(source: &Path, target: &Path) {
    std::fs::create_dir(target).unwrap_or_else(|error| panic!("{}: {error}", target.display()));
    let entries = std::fs::read_dir(source).expect("the directory to copy");
    for entry in entries {
        let from = entry.expect("a directory entry").path();
        let to = target.join(from.file_name().expect("a name"));
        if from.is_dir() {
            copytree(&from, &to);
        } else {
            std::fs::copy(&from, &to).unwrap_or_else(|error| panic!("{}: {error}", from.display()));
            copystat(&from, &to);
        }
    }
    copystat(source, target);
}

/// `shutil.copystat(source, target)`: `target` given the access and modification times of
/// `source`, then its mode.
fn copystat(source: &Path, target: &Path) {
    let metadata = std::fs::metadata(source).expect("the source's metadata");
    let times = FileTimes::new()
        .set_accessed(metadata.accessed().expect("an access time"))
        .set_modified(metadata.modified().expect("a modification time"));
    let copy = File::open(target).expect("the copy");
    copy.set_times(times).expect("the copy's times");
    std::fs::set_permissions(target, metadata.permissions()).expect("the copy's mode");
}

/// The commit the remote's coordination ref names:
/// `git --git-dir ORIGIN rev-parse refs/beads/journal`.
fn journal_tip(sandbox: &Sandbox) -> String {
    let args: [&OsStr; 4] = [
        "--git-dir".as_ref(),
        sandbox.origin().as_os_str(),
        "rev-parse".as_ref(),
        "refs/beads/journal".as_ref(),
    ];
    git(args, None)
}

/// Replay scopes JSON null, but remote_own must use the same validation boundary.
#[test]
fn invalid_own_remote_record_must_not_disable_sync() {
    let sandbox = crate::sandbox();
    let (a, key) = set_up(&sandbox, Machine::new);
    a.bead(["sync"]);
    append(&segment(&a), b"null\n");
    let info = a.bead(["info"]).expect("bead info prints a record");
    let integrity = member(&info, "integrity");
    let rows = integrity.as_array().map(<[Json]>::len);
    assert_eq!(rows, Some(1), "{}", dumps(integrity));
    assert_eq!(member(&a.show(&key), "title"), &json!("storage survivor"));
    let synced = a.bead(["sync"]).expect("bead sync prints a record");
    // publication itself succeeds
    assert_eq!(member(&synced, "integrity"), &json!(1));
    let again = a.run(["sync", "--json"]);
    assert_eq!(
        again.returncode(),
        0,
        "a previously quarantined malformed record now disables sync:\n{}",
        again.stderr
    );
}

#[test]
fn repair_preserves_a_complete_record_that_only_lacks_newline() {
    let sandbox = crate::sandbox();
    let (a, _) = set_up(&sandbox, Machine::new);
    let expected = a.export();
    let segment = segment(&a);
    // `segment.read_bytes().rstrip(b"\n")`
    let mut stripped = bytes(&segment);
    while stripped.last() == Some(&b'\n') {
        stripped.pop();
    }
    std::fs::write(&segment, stripped).expect("the segment without its newline");
    assert_eq!(a.export(), expected);
    a.bead(["create", "after missing separator"]);
    a.bead(["sync"]);
    let listed = a.bead(["list"]).expect("bead list prints a page");
    assert_eq!(member(&listed, "total"), &json!(2));
    let torn = named(parent(&segment), ".torn-");
    assert!(torn.is_empty(), "{torn:?}");
    let info = a.bead(["info"]).expect("bead info prints a record");
    assert_eq!(member(&info, "integrity"), &json!([]));
}

#[test]
fn repair_foreign_fragment_before_publish_retains_authoritative_events() {
    let sandbox = crate::sandbox();
    let (a, _) = set_up(&sandbox, Machine::new);
    let b = two(&sandbox, &a, Machine::new);
    let segment = segment(&a);
    let relative = relative(&segment, &a);
    let foreign = b.store().join(relative);
    append(&foreign, br#"{"v":1,"id":"unfinished"#);
    let before = a.export();
    let result = b.bead(["sync"]).expect("bead sync prints a record");
    assert_eq!(member(&result, "integrity"), &json!(0));
    assert_eq!(a.export(), before);
    assert_eq!(b.export(), before);
    assert_eq!(bytes(&foreign), bytes(&segment));
    let torn = named(parent(&foreign), ".torn-");
    assert_eq!(torn.len(), 1, "{torn:?}");
    let synced = a.bead(["sync"]).expect("bead sync prints a record");
    assert_eq!(member(&synced, "integrity"), &json!(0));
}

#[test]
fn rewritten_foreign_file_preserves_local_backup_and_published_bytes() {
    let sandbox = crate::sandbox();
    let (a, _) = set_up(&sandbox, Machine::new);
    let b = two(&sandbox, &a, Machine::new);
    let segment = segment(&a);
    let relative = relative(&segment, &a);
    let foreign = b.store().join(&relative);
    let original = bytes(&foreign);
    let text = String::from_utf8(original.clone()).expect("a segment is UTF-8");
    let rows = text.lines().map(|line| parse_json(line).expect("an event"));
    let mut rows: Vec<Json> = rows.collect();
    let last = rows.last_mut().expect("the segment's last event");
    retitle(last, "damaged local foreign title");
    let damaged: String = rows.iter().map(|row| dumps(row) + "\n").collect();
    std::fs::write(&foreign, &damaged).expect("the rewritten copy");
    let result = b.bead(["sync"]).expect("bead sync prints a record");
    let published = relative.to_str().expect("a UTF-8 path");
    assert_eq!(member(&result, "forked_journals"), &json!([published]));
    assert_eq!(bytes(&foreign), original);
    let backups = named(parent(&foreign), ".fork-");
    assert_eq!(backups.len(), 1, "{backups:?}");
    assert_eq!(bytes(&backups[0]), damaged.as_bytes());
    let synced = a.bead(["sync"]).expect("bead sync prints a record");
    assert_eq!(member(&synced, "integrity"), &json!(0));
    assert_eq!(a.export(), b.export());
}

#[test]
fn copied_identity_divergence_across_rotation_refuses_remote_change() {
    let sandbox = crate::sandbox();
    let (mut a, key) = set_up(&sandbox, Machine::new);
    let _ = two(&sandbox, &a, Machine::new);
    let mut twin = Machine::new(&sandbox, "identity-twin");
    copytree(a.home(), twin.home());
    a.extra.insert("BEAD_ROTATE_BYTES".into(), "1".into());
    twin.extra
        .insert("BEAD_ROTATE_BYTES".into(), "9999999".into());
    twin.bead(["update", &key, "--title", "twin branch", "--offline"]);
    a.bead(["update", &key, "--title", "original branch", "--offline"]);
    a.bead(["sync"]);
    let tip = journal_tip(&sandbox);
    let attempted = twin.run(["sync", "--json"]);
    assert_eq!(code(&attempted), "JOURNAL_FORK");
    assert_eq!(journal_tip(&sandbox), tip);
    assert_eq!(member(&a.show(&key), "title"), &json!("original branch"));
}
