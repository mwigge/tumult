//! `tumult-compliance` — compliance-grade report documents (v2 pipeline).
//!
//! A renderer-agnostic content model ([`ReportDoc`]) with two outputs: a
//! print-styled HTML preview for the UI and a Typst-compiled PDF. Also home
//! to the resilience scoring model that feeds both the executive digest and
//! `GET /api/scores`.

pub mod builders;
pub mod html;
pub mod markup;
pub mod model;
pub mod org;
pub mod scoring;
pub mod svg;
pub mod typst_pdf;

pub use model::{Block, Cell, ChartSpec, DocMeta, ReportDoc, TemplateKind};
pub use org::{OrgNodeScore, OrgTree, ScoredLeaf};
