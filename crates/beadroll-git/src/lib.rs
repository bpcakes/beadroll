//! The only crate that starts git, and it starts the git binary (`docs/REVIEWING.md` §2). It owns
//! process sessions, timeout handling, output files, persistent object readers and writers,
//! checkout facts, transport environments, push and damage classification, URL normalization and
//! lenient path resolution.
//!
//! Every process runs in an explicit [`Env`]; this crate reads no ambient environment, working
//! directory or clock.

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
mod tree;
mod url;
mod writers;

pub use checkout::{Checkout, Fact, HeadAnswers, ORIGIN_KEYS, head_and_branch};
pub use classify::{
    PushOutcome, local_damage, lost_to_other_fetch, push_outcome, remote_lacks_objects,
};
pub use env::{
    CountRules, DEFAULT_SSH_COMMAND, REPO_LOCATION_ENV, SYNC_REPO_CONFIG, TRANSPORT_KEYS, sync_env,
    transport_config, transport_settings, with_settings,
};
pub use error::{Error, IoError, strerror};
pub use format::{StoredFormat, stored_format};
pub use objects::{Object, Objects, TreeListing, list_tree, list_tree_with_subtrees, root_tree};
pub use path::{exists, realpath};
pub use runner::{Env, Git, KillScope, Output};
pub use session::NoAnswer;
pub use signal::signal_name;
pub use text::path_arg;
pub use tree::blob_tree_id;
pub use url::{
    is_local_path, lowercase_text_bytes, normalize_remote, strip_text_bytes, without_password,
};
pub use writers::{Hasher, Refs, Trees};
