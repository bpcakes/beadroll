//! The names of what a store home holds. The home (`Settings::home`) keeps this machine's identity,
//! the aliases of other machines' and the record of moved workspaces, beside one store per
//! workspace; each store keeps its metadata, its journals, its attempts not yet accepted, its lock,
//! its disposable caches and the bare repository it syncs through. A path to any of them joins its
//! name here, never a literal.

/// In the home: this machine's identity, minted on first use.
pub const REPLICA: &str = "replica.json";

/// In the home: the names other machines' identities are shown by.
pub const ALIASES: &str = "aliases.json";

/// In the home: URL-derived workspace ids whose repository's tracker was found under another id.
pub const MOVED: &str = "moved.json";

/// In a store: what this store last saw of its remote and its publication.
pub const META: &str = "meta.json";

/// In a store: the bare repository its exchange with the remote goes through.
pub const SYNC_GIT: &str = "sync.git";

/// In a store: the journals, one folder per machine, as the remote holds them (format 1).
pub const EVENTS: &str = "events";

/// In a machine's journal folder: its accepted ownership attempts, a file each (format 1).
pub const CLAIMS: &str = "claims";

/// In a store: this machine's ownership attempts the remote has not confirmed, each beside the
/// receipt of the command that made it.
pub const PENDING: &str = "pending";

/// In a store: the file its writers lock.
pub const LOCK: &str = "lock";

/// In a store: the cache of the history `bead log` reads, made again whenever it may be stale.
pub const HISTORY: &str = "history";

/// In a store: the cache of the state its journals replay to.
pub const PROJECTION: &str = "projection";

/// In a store: the texts of its projection, read as an issue needs them.
pub const PROJECTION_TEXTS: &str = "projection-texts";

/// In `sync.git`: the memo of which blob each journal file was published as.
pub const JOURNAL_MEMO: &str = "journal-memo";
