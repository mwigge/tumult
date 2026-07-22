//! `ChaosGraph` — a token-efficient knowledge graph over Tumult's chaos data.
//!
//! This crate is the **pure model layer** of `ChaosGraph`. It knows how to turn
//! a [`tumult_core::types::Journal`] (optionally enriched with its
//! [`tumult_core::types::Experiment`] definition) into a small set of typed
//! graph [`Node`]s and [`Edge`]s, and it owns the SQL used to persist and
//! query those nodes/edges. It holds **no database handle of its own**:
//! `tumult-analytics` owns the embedded `DuckDB` connection and executes the
//! SQL exposed here. That keeps the dependency direction acyclic —
//! `tumult-graph` depends only on `tumult-core`, and `tumult-analytics`
//! depends on `tumult-graph`.
//!
//! # Why a graph?
//!
//! Agents answering "what breaks when I inject latency into demo-app?" do not
//! need a full 2 600-token journal. They need the neighbourhood of a node.
//! The graph collapses each run to a handful of `(src)-[rel]->(dst)` tuples.
//!
//! ## Before / after token illustration
//!
//! **Before** — the agent reads a raw journal to learn what an experiment
//! touched. A representative `demo-net` journal (steady-state hypothesis,
//! method results with full provider output, trace/span ids, analysis block)
//! serialises to roughly **2 600 tokens** of JSON:
//!
//! ```text
//! {
//!   "experiment_title": "Demo — network latency via the tumult-net ...",
//!   "experiment_id": "b1f2...-...",
//!   "status": "completed",
//!   "started_at_ns": 1774980000000000000,
//!   "method_results": [ { "name": "inject-latency", "output": "proxy ...", ... },
//!                       { "name": "health-through-delayed-proxy", ... } ],
//!   "steady_state_before": { ... }, "steady_state_after": { ... },
//!   "analysis": { ... }, ...                      // ~480 tokens, per run
//! }
//! ```
//!
//! **After** — the same question answered by a single `chaosgraph_neighbors`
//! call on the experiment node. A targeted (`rel`-filtered) sub-graph is about
//! **110 tokens**, and — the point — it stays that size no matter how many
//! runs accumulate, whereas the journal corpus grows by ~480 tokens per run:
//!
//! ```text
//! center: exp:Demo — network latency via the tumult-net userspace proxy
//! nodes:
//!   exp:Demo … (experiment)   fault:tumult-net::inject_latency (fault)
//!   svc:demo-app (service)     run:b1f2… (journal: completed)
//! edges:
//!   (exp:Demo …)-[injects]->(fault:tumult-net::inject_latency)
//!   (exp:Demo …)-[targets]->(svc:demo-app)
//!   (exp:Demo …)-[yielded]->(run:b1f2…)
//!   (fault:tumult-net::inject_latency)-[observed_on]->(svc:demo-app)
//! ```
//!
//! Per run of history: the graph grows by one small node (~a few hundred
//! bytes) while the raw journal corpus grows by a full ~480-token journal —
//! roughly 8x more compact per run, ~20x on store-wide queries, and bounded
//! for targeted questions. All reproducible via `make demo-proof`.

pub mod attribution;
pub mod compliance;
pub mod coverage;
pub mod lineage;
mod map;
mod model;
pub mod recommend;
pub mod render;
mod service;
pub mod sql;
pub mod topology;

pub use compliance::{compliance_article_id, compliance_article_nodes, resolve_citation};
pub use coverage::{coverage_gap_delta, AvailableAction, COVERAGE_GAP_RUN_ID};
pub use map::journal_to_graph;
pub use model::{
    Edge, EdgeRecord, EdgeRel, EgoGraph, EgoTuple, GraphDelta, Node, NodeKind, NodeSummary,
};
pub use topology::{
    parse_topology, topology_delta, TopologyDoc, TopologyError, TopologyService, TOPOLOGY_RUN_ID,
};
