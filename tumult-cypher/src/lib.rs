//! openCypher query surface over tumult's `ChaosGraph`.
//!
//! # Why this crate exists
//!
//! Agents (via tumult-mcp / tumult-agentic) want to ask graph-shaped questions
//! ("which experiments injected faults observed on services my service depends
//! on?") that are awkward to express as recursive SQL over the `DuckDB` edge
//! table. openCypher is the lingua franca for that kind of traversal.
//!
//! # Architecture: no write-path mirror
//!
//! `DuckDB` stays the *only* source of truth. This crate deliberately does NOT
//! maintain a live graph replica that would have to be kept consistent with
//! every `ChaosGraph` write. Instead, the caller extracts the rows it cares
//! about into a [`GraphSnapshot`], and [`run_cypher`] rebuilds a throwaway
//! in-memory [GrafeoDB](https://grafeo.dev) graph, runs the query, converts
//! the result table to JSON values, and drops the engine. Rebuild cost is
//! linear in snapshot size, which is acceptable because `ChaosGraph` snapshots
//! are small (thousands of rows, not millions) and it buys us zero
//! consistency machinery.
//!
//! # Read-only enforcement
//!
//! The snapshot is disposable, so a mutating query could not corrupt anything
//! durable — but allowing it would let an agent silently "succeed" at writes
//! that vanish, which is worse than an error. Mutating clauses are therefore
//! rejected before execution; see the guard in `engine.rs` for the
//! (conservative, documented) token-scan limitation.

mod engine;
mod snapshot;

pub use engine::{run_cypher, run_cypher_capped, CypherError, CypherTable, DEFAULT_ROW_CAP};
pub use snapshot::{GraphSnapshot, SnapshotEdge, SnapshotNode};
