//! The machine-local store: explicit settings, BEAD_HOME files and
//! permissions, atomic writes, journals and pending attempts, journal memo, identity, aliases,
//! workspace discovery, remote resolution, actors and command views.
//!
//! Ambient environment, directory and clock values arrive through [`Settings`]. Persistent
//! tracker bytes obey format 1; memo and projection files are disposable caches.

mod actor;
mod discover;
mod elsewhere;
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
pub use elsewhere::{Elsewhere, elsewhere};
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
    Clock, NET_TIMEOUT, REPACK_LOOSE, REPACK_PACKS, ROTATE_BYTES, Settings, join_path,
    normalize_path,
};
pub use workspace::{DEFAULT_REF, Endpoint, REMOTE_REF, Workspace, relative, remembered, stamp};

/// The bytes without the ASCII whitespace at either end.
pub(crate) fn strip_ascii(data: &[u8]) -> &[u8] {
    beadroll_core::text::strip_bytes(data)
}
