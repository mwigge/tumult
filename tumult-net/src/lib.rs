//! Tumult Net — a TCP chaos-proxy plugin built on
//! [`tokio-netem`](https://docs.rs/tokio-netem).
//!
//! `tumult-net` forwards TCP traffic through a userspace proxy and injects
//! directional faults on the forwarded stream using the `tokio-netem` I/O
//! adapters — no `tc`, `iptables`, or `NET_ADMIN` privileges required.
//!
//! # Supported chaos actions
//!
//! - **Latency** — one-way delay with deterministic jitter (`inject_latency`)
//! - **Bandwidth throttle** — leaky-bucket egress limit (`throttle_bandwidth`)
//! - **Fragmentation** — fixed-size write slicing / MTU emulation (`fragment_stream`)
//! - **Corruption** — seeded per-byte bit-flips (`corrupt_bytes`)
//! - **Termination** — seeded probabilistic mid-stream hard close (`terminate_connections`)
//! - **Composite** — all of the above at once (`start_proxy`)
//!
//! Every disruptive action is rolled back by [`actions::stop_proxy`].
//!
//! # Probes
//!
//! - TCP reachability (`probes::reachable`)
//! - Handshake latency in milliseconds (`probes::measured_latency`)
//!
//! # Determinism
//!
//! A `seed` governs the reproducible fault schedule: the jitter offset added to
//! the latency knob, and — via `tokio-netem`'s `from_seed` constructors — the
//! byte-corruption and termination RNGs. Given the same seed and traffic, the
//! same bytes are flipped and the same connections are killed.

pub mod actions;
pub mod config;
pub mod error;
pub(crate) mod faults;
pub mod handles;
pub mod probes;
pub mod proxy;
pub(crate) mod telemetry;

pub use config::{FaultProfile, ProxySpec};
pub use error::NetError;
pub use handles::FaultHandles;
pub use proxy::Proxy;
