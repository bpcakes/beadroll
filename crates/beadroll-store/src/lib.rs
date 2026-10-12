//! This machine's side of `bead` (`docs/RUST-PORT.md` §2.1): the settings the CLI hands down, the
//! store under `BEAD_HOME` with its files and modes, atomic writes, journal segments and torn
//! tails, attempt and pending files, what the store remembers of its journal files between commands
//! (`Memo`), this machine's identity and aliases, workspace discovery and remote resolution, actor
//! detection, and `view`, the state a command reads.
//!
//! Nothing here reads the process environment, the working directory or the clock offset: they
//! arrive in [`Settings`]. The bytes on disk are format 1's, exactly as Python writes them (§1.1).

mod actor;
mod discover;
mod error;
mod failpoint;
mod fs;
mod identity;
mod journal;
mod memo;
mod place;
mod projection;
mod settings;
mod workspace;

pub use actor::{Actor, Label, Ps, detect_actor, nearest_harness};
pub use discover::{
    checked_config, derived_shaped, find_workspace, moved_workspaces, tracked_config_warning,
    workspaces_named,
};
pub use error::{Error, git_code};
pub use failpoint::{Action, Failpoints, Fired, POINTS, Selector, kill_self};
pub use fs::{
    Unreadable, dir_entries, fsync, glob_ending, has_suffix, json_text, os_error_str, private_dir,
    private_dirs, read, read_json_keeping_surrogates, read_json_text, utf8_error, write_atomic,
};
pub use identity::{hex_token, machine_aliases, replica_id, uuid7};
pub use journal::{
    ViewError, active_segment, append_events, attempt_path, claim_files, complete_prefix,
    encode_events, extend_segment, load_journal, load_pending, pending_files, read_attempt, remove,
    repair_tail, segment_files, view, view_outline, write_attempt_file,
};
pub use memo::{FileKey, Memo, memo_path};
pub use place::{capture_where, merged_into_default, object_format};
pub use projection::{projection, projection_path, read_texts, texts_path};
pub use settings::{
    Clock, NET_TIMEOUT, REPACK_LOOSE, REPACK_PACKS, ROTATE_BYTES, Settings, pathlib, pathlib_join,
};
pub use workspace::{DEFAULT_REF, Endpoint, REMOTE_REF, Workspace, relative, remembered, stamp};

/// `data.strip()` on bytes: without the ASCII whitespace at either end.
pub(crate) fn strip_ascii(data: &[u8]) -> &[u8] {
    beadroll_core::text::strip_bytes(data)
}
