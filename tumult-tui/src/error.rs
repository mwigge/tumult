//! Error type for the TUI's fallible operations (store access, terminal I/O).
//!
//! Binaries launching the TUI (e.g. `tumult tui` in `tumult-cli`) map this
//! into `anyhow` at their boundary.

use std::path::PathBuf;

use tumult_lake::AnalyticsError;

/// Errors from launching or driving the analytics TUI.
#[derive(Debug, thiserror::Error)]
pub enum TuiError {
    /// The default analytics store path could not be determined.
    #[error("cannot determine the default analytics store: {0}")]
    DefaultStorePath(#[source] AnalyticsError),

    /// No analytics store exists at the resolved path.
    #[error(
        "no analytics store at {}\n\
         Run an experiment first (e.g. `tumult run experiment.toon`) to create it, \
         or pass --store <path>.",
        .0.display()
    )]
    StoreMissing(PathBuf),

    /// The store could not be opened read-only (e.g. a writer holds the
    /// exclusive lock).
    #[error("opening analytics store read-only at {}: {source}", path.display())]
    OpenReadOnly {
        path: PathBuf,
        source: AnalyticsError,
    },

    /// The store stats query failed.
    #[error("reading store stats: {0}")]
    StoreStats(#[source] AnalyticsError),

    /// The experiments history query failed.
    #[error("querying experiments history: {0}")]
    ExperimentsHistory(#[source] AnalyticsError),

    /// The activity timeline query failed.
    #[error("querying activity timeline: {0}")]
    ActivityTimeline(#[source] AnalyticsError),

    /// The `ChaosGraph` node query failed.
    #[error("querying ChaosGraph nodes of kind {kind}: {source}")]
    GraphNodes {
        kind: String,
        source: AnalyticsError,
    },

    /// The `ChaosGraph` neighbour query failed.
    #[error("querying ChaosGraph neighbours: {0}")]
    GraphNeighbours(#[source] AnalyticsError),

    /// Any other store failure (e.g. opening the store read-only outside the
    /// initial snapshot load).
    #[error(transparent)]
    Store(#[from] AnalyticsError),

    /// Terminal draw/poll/read failure.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
