//! Error type for the kronika store.

use std::path::PathBuf;

/// Errors raised by [`crate::Store`], [`crate::Writer`] and [`crate::Reader`].
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// Another process holds the store open read-write (or a writer blocks
    /// readers). DuckDB is single-writer per file; see the crate docs.
    #[error(
        "store at {} is locked by another process holding it open read-write; \
         stop the other process or use a different KRONIKA_DB path",
        path.display()
    )]
    StoreLocked { path: PathBuf },

    #[error(transparent)]
    Duckdb(#[from] duckdb::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error("{0}")]
    Internal(String),
}
