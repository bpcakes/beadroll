//! The store lock is let go of when its guard drops, whatever else still holds its open file.

use std::fs::File;
use std::sync::Arc;

use beadroll_core::json::Object;

use crate::layout;
use crate::settings::Settings;
use crate::workspace::Workspace;

/// A guard dropped while another descriptor of its open file is still open, as a child that
/// another thread is starting holds one until its exec closes it, leaves the store unlocked at
/// once (roll-x9x5). Let go of by the close alone, a lock taken with `flock` lasts until that
/// descriptor closes too, and the next locker finds it held.
#[test]
fn a_dropped_guard_unlocks_though_a_copy_of_its_file_is_still_open() {
    let home = tempfile::tempdir().unwrap();
    let settings = Arc::new(Settings::at_home(home.path(), None));
    let ws = Workspace::new(settings, "store", None, None, Object::new()).unwrap();
    let guard = ws.lock().unwrap();
    let copy = guard.file.try_clone().unwrap();
    drop(guard);
    let probe = File::options()
        .read(true)
        .write(true)
        .open(ws.dir.join(layout::LOCK))
        .unwrap();
    probe
        .try_lock()
        .expect("unlocked though a copy of the guard's file is open");
    drop(copy);
}
