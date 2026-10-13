//! Cached names have readdir's answers after every directory change, while a file rewritten in
//! place keeps the names and changes the FileKey that projection and blob reuse check.

use std::os::unix::fs::PermissionsExt as _;
use std::sync::Arc;

use crate::memo::FileKey;

use super::{Claims, DirectoryKey};

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

#[test]
fn in_place_rewrites_reuse_names_but_never_file_identity() {
    let home = tempfile::tempdir().unwrap();
    let folder = home.path().join("a/claims");
    std::fs::create_dir_all(&folder).unwrap();
    let path = folder.join("1.json");
    std::fs::write(&path, b"before").unwrap();
    let mut claims = Claims::default();
    let files = claims.files(home.path()).unwrap();
    let directory = DirectoryKey::of(&folder).unwrap();
    let file = FileKey::of(&path).unwrap();
    std::fs::write(&path, b"AFTER!").unwrap();
    assert_eq!(DirectoryKey::of(&folder), Some(directory));
    assert_ne!(FileKey::of(&path), Some(file));
    assert!(Arc::ptr_eq(&files, &claims.files(home.path()).unwrap()));
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
