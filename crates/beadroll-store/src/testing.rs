//! What the store's unit tests share.

use std::path::Path;
use std::time::SystemTime;

/// Sets the modification time of the file or directory `path` to `time`: a listing's or a
/// projection's view of it moved behind or past a change, whatever the clock an inode's times
/// come from has done since. Setting a time needs the owner, not write access, so the path is
/// opened to read, which a directory can be too.
pub(crate) fn set_modified(path: &Path, time: SystemTime) {
    let opened =
        std::fs::File::open(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    opened
        .set_times(std::fs::FileTimes::new().set_modified(time))
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
}
