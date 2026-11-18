//! The names of what a store home holds. The home (`Settings::home`) keeps this machine's identity,
//! the aliases of other machines' and the record of moved workspaces, beside one store per
//! workspace; each store keeps its metadata and the bare repository it syncs through.

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
