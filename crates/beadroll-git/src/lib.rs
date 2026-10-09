//! The only code in `bead` that starts git (`docs/RUST-PORT.md` §2.1, §2.3d, §2.3f): the runner
//! (its own session, a timeout that kills the process group or only the client, output captured to
//! unlinked files, signal names), the environment `sync.git`'s git runs in, the push-outcome and
//! local-damage classifiers, the remote URL rules, and the path rules they share with the store
//! (Python's lenient `realpath`).
//!
//! Nothing here reads the process environment, the working directory or the clock: every git runs
//! in an [`Env`] its caller built.

mod classify;
mod env;
mod error;
mod path;
mod runner;
mod signal;
mod text;
mod url;

pub use classify::{PushOutcome, local_damage, push_outcome, remote_lacks_objects};
pub use env::{
    CountRules, DEFAULT_SSH_COMMAND, REPO_LOCATION_ENV, SYNC_REPO_CONFIG, TRANSPORT_KEYS, sync_env,
    transport_config,
};
pub use error::{Error, IoError, strerror};
pub use path::{exists, realpath};
pub use runner::{Env, Git, Output};
pub use signal::signal_name;
pub use text::path_arg;
pub use url::{is_local_path, normalize_remote, py_lower, py_strip, without_password};
