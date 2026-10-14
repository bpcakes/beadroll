//! Cached names have readdir's answers after every directory change, while a file rewritten in
//! place keeps the names: what its bytes are is for the FileKey that projection and blob reuse
//! check, and for the sums those caches keep, to say.

use std::fs::FileTimes;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::memo::FileKey;

use super::{Claims, DirectoryKey};

/// Puts `dir`'s modification time back where the listing it was taken for cannot have seen it,
/// so that a change made here is seen whatever the clock an inode's times come from has done: a
/// directory that gains or loses a file within one of that clock's ticks keeps its identity, and
/// what bead writes itself is listed again because the write forgets the names (Y01).
fn aged(dir: &Path) {
    let earlier = SystemTime::now() - Duration::from_secs(60);
    let opened = std::fs::File::open(dir).unwrap();
    opened
        .set_times(FileTimes::new().set_modified(earlier))
        .unwrap();
}

#[test]
fn names_are_shared_until_added_removed_or_replaced() {
    let home = tempfile::tempdir().unwrap();
    let events = home.path().join("events");
    let a = events.join("a/claims");
    let b = events.join("b/claims");
    let mut claims = Claims::default();
    assert!(claims.files(&events).unwrap().is_empty());
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    std::fs::write(a.join("2.json"), b"two").unwrap();
    std::fs::write(a.join("1.json"), b"one").unwrap();
    std::fs::write(b.join("3.json"), b"three").unwrap();
    let first = claims.files(&events).unwrap();
    assert_eq!(
        &*first,
        &[a.join("1.json"), a.join("2.json"), b.join("3.json")]
    );
    assert!(Arc::ptr_eq(&first, &claims.files(&events).unwrap()));
    let unchanged = Arc::clone(&claims.directories[&b].1);

    std::fs::remove_file(a.join("2.json")).unwrap();
    std::fs::write(a.join("4.json"), b"four").unwrap();
    std::fs::write(a.join("ignored.fork"), b"fork").unwrap();
    aged(&a);
    let changed = claims.files(&events).unwrap();
    assert_eq!(
        &*changed,
        &[a.join("1.json"), a.join("4.json"), b.join("3.json")]
    );
    assert!(Arc::ptr_eq(&unchanged, &claims.directories[&b].1));
    assert!(!Arc::ptr_eq(&first, &changed));

    std::fs::rename(&a, home.path().join("old-claims")).unwrap();
    std::fs::create_dir(&a).unwrap();
    std::fs::write(a.join("5.json"), b"five").unwrap();
    assert_eq!(
        &*claims.files(&events).unwrap(),
        &[a.join("5.json"), b.join("3.json")]
    );
    std::fs::remove_dir_all(events.join("b")).unwrap();
    assert_eq!(&*claims.files(&events).unwrap(), &[a.join("5.json")]);
}

/// A rewrite in place keeps a directory's identity, so the names are reused; it changes the
/// file's, as long as the times it sets are not the ones the file already has. A kernel stamps an
/// inode from a clock of its own granularity, so a same-length rewrite within one of its ticks
/// keeps every part of the identity: a file of a moment's own tick is not settled before it
/// ([`FileKey::settled_before`]), and no cache may trust what it remembers of such a file.
#[test]
fn in_place_rewrites_reuse_names_and_a_file_key_never_vouches_for_bytes() {
    let home = tempfile::tempdir().unwrap();
    let folder = home.path().join("a/claims");
    std::fs::create_dir_all(&folder).unwrap();
    let path = folder.join("1.json");
    std::fs::write(&path, b"before").unwrap();
    let mut claims = Claims::default();
    let files = claims.files(home.path()).unwrap();
    let directory = DirectoryKey::of(&folder).unwrap();
    // written long enough ago for any clock an inode's times come from
    let earlier = SystemTime::now() - Duration::from_secs(60);
    let opened = std::fs::File::options().write(true).open(&path).unwrap();
    opened
        .set_times(FileTimes::new().set_modified(earlier))
        .unwrap();
    drop(opened);
    let file = FileKey::of(&path).unwrap();
    std::fs::write(&path, b"AFTER!").unwrap();
    assert_eq!(DirectoryKey::of(&folder), Some(directory));
    assert_ne!(FileKey::of(&path), Some(file), "a write of its own moment");
    assert!(Arc::ptr_eq(&files, &claims.files(home.path()).unwrap()));
    // the key of a write is never settled before that write's own moment
    let now = FileKey::of(&path).unwrap();
    assert!(!now.settled_before(now.written()), "of that moment's tick");
    assert!(now.settled_before(now.written() + 1_000_000_000));
}

#[test]
fn an_unreadable_or_non_directory_is_not_hidden_by_cached_names() {
    let home = tempfile::tempdir().unwrap();
    let folder = home.path().join("a/claims");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("1.json"), b"one").unwrap();
    let mut claims = Claims::default();
    assert_eq!(claims.files(home.path()).unwrap().len(), 1);
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o0)).unwrap();
    // Root can still read the directory; other users must get the same error as read_dir.
    if std::fs::read_dir(&folder).is_err() {
        assert!(claims.files(home.path()).is_err());
    }
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::remove_dir_all(&folder).unwrap();
    std::fs::write(&folder, b"a file").unwrap();
    assert!(claims.files(home.path()).is_err());
    std::fs::remove_file(&folder).unwrap();
    assert!(claims.files(home.path()).unwrap().is_empty());
}
