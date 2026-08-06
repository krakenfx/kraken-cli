//! The crate's failure modes. Deliberately three-way — "you may not"
//! (`Rejected`), "your data is broken" (`Damaged`), "the machine failed"
//! (`Engine`/`Io`) — because that is the distinction consumers route on:
//! retry policy, user messaging, and the CLI's stable envelope categories
//! all map onto it one-to-one. (Plain names, not links: `Engine` only
//! exists under the `duckdb` feature, and a doc link to a gated variant
//! breaks the doc build without it.)

/// Alias for the recording layer, like `io::Result`.
pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A store that can't be opened or written as asked: held by another
    /// process, missing, foreign, or version-incompatible. Refused up front —
    /// nothing was read or written.
    #[error("{0}")]
    Rejected(String),
    /// Content the schema contract can't decode — or a value it can't store.
    #[error("{0}")]
    Damaged(String),
    /// The embedded engine failed — local I/O from the caller's perspective.
    #[cfg(feature = "duckdb")]
    #[error(transparent)]
    Engine(#[from] duckdb::Error),
    /// Filesystem failure around the store (parent dirs, locks, sidecars).
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
