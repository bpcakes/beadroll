//! The only code in `bead` that starts git (`docs/RUST-PORT.md` §2.1, §2.3d, §2.3f): the runner
//! (its own session, a timeout that kills the process group or only the client, output captured to
//! unlinked files, signal names), the long-lived gits Phase 4 keeps (`cat-file --batch-command`
//! with git's own tree and commit reading mirrored in-process, `hash-object --stdin-paths`, `mktree
//! --batch`, `update-ref --stdin`), the checkout's facts asked once, the environment `sync.git`'s
//! git runs in, the push-outcome and local-damage classifiers, the remote URL rules, and the path
//! rules they share with the store (Python's lenient `realpath`).
//!
//! Nothing here reads the process environment, the working directory or the clock: every git runs
//! in an [`Env`] its caller built.

mod alongside;
mod checkout;
mod classify;
mod env;
mod error;
mod format;
mod hash;
mod objects;
mod path;
mod runner;
mod session;
mod signal;
#[cfg(test)]
mod testing;
mod text;
mod url;
mod writers;

pub use checkout::{Checkout, Fact, HeadAnswers, ORIGIN_KEYS, head_and_branch};
pub use classify::{PushOutcome, local_damage, push_outcome, remote_lacks_objects};
pub use env::{
    CountRules, DEFAULT_SSH_COMMAND, REPO_LOCATION_ENV, SYNC_REPO_CONFIG, TRANSPORT_KEYS, sync_env,
    transport_config, transport_settings,
};
pub use error::{Error, IoError, strerror};
pub use format::{StoredFormat, stored_format};
pub use objects::{Object, Objects, list_tree, root_tree};
pub use path::{exists, realpath};
pub use runner::{Env, Git, Output};
pub use session::NoAnswer;
pub use signal::signal_name;
pub use text::path_arg;
pub use url::{is_local_path, normalize_remote, py_lower, py_strip, without_password};
pub use writers::{Hasher, Refs, Trees};
