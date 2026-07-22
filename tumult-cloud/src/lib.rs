//! Tumult Cloud — thin connectors to cloud providers' own fault/chaos APIs.
//!
//! This crate does **not** reimplement cloud faults. It drives each provider's
//! own managed fault service (where one exists) through a small, signed HTTP
//! client, and adds a couple of direct high-signal faults that don't need a
//! preconfigured experiment template.
//!
//! # Providers
//!
//! - **AWS Fault Injection Service (FIS)** — start / stop / status an
//!   experiment template, plus direct EC2 instance stop / terminate. Requests
//!   are SigV4-signed ([`sigv4`]) against `fis.<region>.amazonaws.com` and
//!   `ec2.<region>.amazonaws.com`. Credentials come from the standard AWS
//!   environment chain (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, optional
//!   `AWS_SESSION_TOKEN`).
//! - **Azure Chaos Studio** — start / cancel / status a Chaos experiment via
//!   the Azure Resource Manager REST API, authenticated with a bearer token
//!   (`AZURE_ACCESS_TOKEN`).
//! - **Google Cloud** — GCP has **no** first-party managed chaos service, so
//!   only a direct Compute Engine instance stop is provided
//!   (`GOOGLE_OAUTH_ACCESS_TOKEN`). See [`gcp`] for the scope note.
//!
//! # Design
//!
//! A plain `reqwest` client plus a hand-rolled `SigV4` signer is used in
//! preference to the `aws-sdk-*` crates, to keep the single-binary footprint
//! small. See the crate `README.md` for the full rationale, the required IAM /
//! RBAC permissions per function, and what is hermetically proven versus what
//! needs real cloud credentials.
//!
//! # Credentials
//!
//! No secret is ever hardcoded. Every connector resolves its credentials from
//! the environment and fails fast — before any network call — with a message
//! naming the exact missing variable (see [`creds`]).

pub mod aws;
pub mod azure;
pub mod creds;
pub mod error;
pub mod gcp;
mod http;
pub mod native;
pub mod sigv4;

pub use error::CloudError;
pub use native::CloudExecutor;
