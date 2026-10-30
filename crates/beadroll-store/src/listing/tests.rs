//! Cached names have readdir's answers after every directory change, while a file rewritten in
//! place keeps the names: what its bytes are is for the FileKey that projection and blob reuse
//! check, and for the sums those caches keep, to say.

use std::fs::FileTimes;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use super::{Claims, DirectoryKey};
use crate::LockState;
use crate::memo::FileKey;

/// Puts `dir`'s modification time back where the listing it was taken for cannot have seen it,
/// so that a change made here is seen whatever the clock an inode's times come from has done: a
/// directory that gains or loses a file within one of that clock's ticks keeps its identity, and
/// what bead writes itself is listed again because the write forgets the names (Y01).
fn aged(dir: &Path) {
    aged_by(dir, 60);
}

/// [`aged`] by `seconds`: a directory aged by a different number of seconds each time has a new
/// identity each time, whatever granularity its clock has.
fn aged_by(dir: &Path, seconds: u64) {
    let earlier = SystemTime::now() - Duration::from_secs(seconds);
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

/// The stamp of a listing is the identity each claims directory had when it was listed, and moves
/// with a directory whoever wrote in it: a file renamed over keeps the names and not the
/// directory's identity, as one added or removed does, and a claims directory that was not there
/// and is, with or without the phase being told (roll-lqjj.37).
#[test]
fn a_stamp_follows_each_claims_directory_whatever_wrote_in_it() {
    let home = tempfile::tempdir().unwrap();
    let events = home.path().join("events");
    let (a, b) = (events.join("a/claims"), events.join("b"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    std::fs::write(a.join("1.json"), b"one").unwrap();
    let mut claims = Claims::default();
    claims.files(&events).unwrap();
    let first = claims.stamp().expect("stable directories");
    let identities: Vec<_> = first
        .iter()
        .map(|(path, key)| (path, key.is_some()))
        .collect();
    assert_eq!(identities, [(&a, true), (&b.join("claims"), false)]);
    // the names are shared, and so is the stamp
    claims.files(&events).unwrap();
    assert_eq!(claims.stamp(), Some(Arc::clone(&first)));

    // a file renamed over, the phase not told: the names are the same, the directory is not
    let names = claims.files(&events).unwrap();
    std::fs::write(a.join("1.json.new"), b"one").unwrap();
    std::fs::rename(a.join("1.json.new"), a.join("1.json")).unwrap();
    aged(&a);
    assert_eq!(*claims.files(&events).unwrap(), *names);
    let second = claims.stamp().unwrap();
    assert_ne!(second, first);

    // a file added and one removed, the phase not told, then told of another write
    std::fs::write(a.join("2.json"), b"two").unwrap();
    aged(&a);
    claims.files(&events).unwrap();
    let third = claims.stamp().unwrap();
    assert_ne!(third, second);
    std::fs::remove_file(a.join("2.json")).unwrap();
    aged(&a);
    claims.forget(&a.join("2.json"));
    claims.files(&events).unwrap();
    assert_ne!(claims.stamp().unwrap(), third);

    // a claims directory that was not there
    let before = claims.stamp().unwrap();
    std::fs::create_dir(b.join("claims")).unwrap();
    claims.files(&events).unwrap();
    assert_ne!(claims.stamp().unwrap(), before);
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

/// Under the store lock a file's identity is taken once a phase: a new listing carries over what
/// the last one took, file by file, but for a file bead wrote since, which is stat-ed again, and a
/// new file, stat-ed for the first time. Outside the lock each is stat-ed every time.
#[test]
fn identities_are_kept_across_listings_but_for_what_bead_writes() {
    let home = tempfile::tempdir().unwrap();
    let folder = home.path().join("a/claims");
    std::fs::create_dir_all(&folder).unwrap();
    let (one, two, three) = (
        folder.join("1.json"),
        folder.join("2.json"),
        folder.join("3.json"),
    );
    std::fs::write(&one, b"one").unwrap();
    std::fs::write(&two, b"two").unwrap();
    let mut claims = Claims::default();
    let files = claims.files(home.path()).unwrap();
    let first = claims.keys(&files, LockState::Locked);
    assert_eq!(&*first, &[FileKey::of(&one), FileKey::of(&two)]);
    // something other than bead moves both files' times: kept, the identities taken stand
    for path in [&one, &two] {
        let opened = std::fs::File::options().write(true).open(path).unwrap();
        let earlier = SystemTime::now() - Duration::from_secs(60);
        opened
            .set_times(FileTimes::new().set_modified(earlier))
            .unwrap();
    }
    assert!(Arc::ptr_eq(&first, &claims.keys(&files, LockState::Locked)));
    assert_ne!(
        claims.keys(&files, LockState::Unlocked)[0],
        first[0],
        "stat-ed outside the lock"
    );
    // bead replaces one and adds another: listed again, those two stat-ed, the other carried over
    std::fs::write(&one, b"ONE").unwrap();
    claims.forget(&one);
    std::fs::write(&three, b"three").unwrap();
    claims.forget(&three);
    let files = claims.files(home.path()).unwrap();
    let keys = claims.keys(&files, LockState::Locked);
    assert_eq!(
        &*keys,
        &[FileKey::of(&one), first[1], FileKey::of(&three)],
        "only what bead wrote is stat-ed again"
    );
    assert_ne!(keys[1], FileKey::of(&two));
}

/// A machine whose claims directory is not there (this one, until it first claims) does not make
/// the combined listing again at every call: its names are the one empty listing, so that the
/// listing kept is the sources' for as long as they stay (roll-lqjj.47, where 92,000 paths were
/// copied at each call), and the directory is seen the moment it has a file.
#[test]
fn a_claims_directory_that_is_not_there_keeps_the_listing() {
    let home = tempfile::tempdir().unwrap();
    let events = home.path().join("events");
    let (a, b) = (events.join("a/claims"), events.join("b"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    std::fs::write(a.join("1.json"), b"one").unwrap();
    let mut claims = Claims::default();
    let first = claims.files(&events).unwrap();
    assert_eq!(&*first, &[a.join("1.json")]);
    for _ in 0..3 {
        assert!(
            Arc::ptr_eq(&first, &claims.files(&events).unwrap()),
            "b has no claims directory, and nothing changed"
        );
    }
    // afresh reads every directory again, and the listing it makes is the same names
    let again = claims.files_afresh(&events).unwrap();
    assert_eq!(&*again, &*first);
    assert!(Arc::ptr_eq(&again, &claims.files(&events).unwrap()));

    // b's directory comes, with a file: seen at once, and kept in its turn
    std::fs::create_dir(b.join("claims")).unwrap();
    std::fs::write(b.join("claims/2.json"), b"two").unwrap();
    let grown = claims.files(&events).unwrap();
    assert_eq!(&*grown, &[a.join("1.json"), b.join("claims/2.json")]);
    assert!(Arc::ptr_eq(&grown, &claims.files(&events).unwrap()));

    // and gone again: the names are those of the directories that are there
    std::fs::remove_dir_all(b.join("claims")).unwrap();
    assert_eq!(&*claims.files(&events).unwrap(), &[a.join("1.json")]);
}

/// A seeded generator (SplitMix64): the same walk on every run.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        ((z ^ (z >> 31)) % n as u64) as usize
    }
}

/// The claims files of the tree under `events` as a walk of it gives them: each machine's in the
/// order of its names, the machines in the order of theirs.
fn walked(events: &Path) -> Vec<PathBuf> {
    let sorted = |dir: &Path| -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        paths.sort();
        paths
    };
    let mut files = Vec::new();
    for machine in sorted(events) {
        let claims = machine.join("claims");
        if machine.is_dir() && claims.is_dir() {
            files.extend(sorted(&claims).into_iter().filter(|file| {
                let name = file.file_name().unwrap().to_string_lossy();
                name.len() > ".json".len() && name.ends_with(".json")
            }));
        }
    }
    files
}

/// A random walk over the claims directories of five machines, whose directories are made, filled,
/// emptied, removed and made again, with the listing asked for after every step: it is the names
/// the tree holds however the walk got there, and it is kept, the very listing, when nothing
/// changed since it was made (roll-lqjj.47: a machine with no claims directory is one empty
/// listing, not a new one at every call).
#[test]
fn a_random_walk_over_claims_directories_lists_what_the_tree_holds() {
    let home = tempfile::tempdir().unwrap();
    let events = home.path().join("events");
    std::fs::create_dir_all(&events).unwrap();
    let mut claims = Claims::default();
    let mut rng = Rng(0x1234_5678_9abc_def1);
    let machines = ["a", "a-b", "b", "c", "d"];
    let mut listed_files = 0;
    for step in 0..4000_u64 {
        let machine = events.join(machines[rng.below(machines.len())]);
        let folder = machine.join("claims");
        let mut changed = false;
        match rng.below(10) {
            0 => std::fs::create_dir_all(&machine).unwrap(),
            1 => std::fs::create_dir_all(&folder).unwrap(),
            2 | 3 => {
                std::fs::create_dir_all(&folder).unwrap();
                let file = folder.join(format!("{}.json", rng.below(12)));
                std::fs::write(&file, b"x").unwrap();
                changed = true;
                // half of what is written is a write bead made and tells the listing of
                if rng.below(2) == 0 {
                    claims.forget(&file);
                }
            }
            4 => {
                let files = walked(&events)
                    .into_iter()
                    .filter(|file| file.starts_with(&folder))
                    .collect::<Vec<_>>();
                if !files.is_empty() {
                    std::fs::remove_file(&files[rng.below(files.len())]).unwrap();
                    changed = true;
                }
            }
            5 => drop(std::fs::remove_dir_all(&folder)),
            6 => drop(std::fs::remove_dir_all(&machine)),
            7 if folder.is_dir() => {
                std::fs::write(folder.join("note.txt"), b"x").unwrap();
                changed = true;
            }
            8 => {
                claims.files_afresh(&events).unwrap();
            }
            _ => {}
        }
        if changed {
            aged_by(&folder, 60 + step);
        }
        let listed = claims.files(&events).unwrap();
        assert_eq!(&*listed, &walked(&events)[..], "step {step}");
        assert!(
            Arc::ptr_eq(&listed, &claims.files(&events).unwrap()),
            "step {step}: nothing changed since the listing"
        );
        listed_files += usize::from(!listed.is_empty());
    }
    assert!(listed_files > 400, "the walk lists files: {listed_files}");
}
