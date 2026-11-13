//! Plain input rows for [`super::assemble`]. The API handler fills these
//! from the lake, the risk scanner, the manifests directory and `org.yaml`;
//! nothing here knows where a row came from.

use std::collections::BTreeMap;

/// One unit of the organisation tree. The root has an empty `path`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OrgUnitRow {
    /// Slash-separated path from the root, for example `platform/edge/edge-team`.
    pub path: String,
    pub name: String,
    /// `company`, `domain`, `unit`, `team` or `unassigned`.
    pub kind: String,
}

/// One assignment glob from `org.yaml`, in file order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GlobRow {
    pub pattern: String,
    /// Path of the org unit the pattern assigns to.
    pub path: String,
}

/// One distinct resource-attribute tuple a service reported in the window.
/// A service that reports two entity ids yields two rows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ServiceRow {
    pub service: String,
    pub entity_id: Option<String>,
    pub entity_tier: Option<String>,
    pub entity_owner: Option<String>,
    pub system: Option<String>,
    pub component: Option<String>,
    pub environment: Option<String>,
    pub k8s_namespace: Option<String>,
    pub k8s_deployment: Option<String>,
    pub version: Option<String>,
    pub spans: u64,
    pub last_seen_ns: i64,
    /// Telemetry-contract conformance level of the service, when known.
    pub conformance: Option<String>,
}

/// Observed calls between two services (parent and child span).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CallRow {
    pub client: String,
    pub server: String,
    pub calls: u64,
    pub errors: u64,
    pub avg_duration_ns: f64,
}

/// Posture of one system, copied from the posture computation.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PostureRow {
    pub score: f64,
    pub band: String,
}

/// The latest run of one experiment against a system.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExperimentRow {
    pub name: String,
    pub kind: String,
    pub passed: bool,
    pub last_run_ns: i64,
}

/// One tested (or manually exercised) system with its coverage row.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SystemRow {
    pub system: String,
    pub environment: Option<String>,
    pub criticality: Option<String>,
    pub posture: Option<PostureRow>,
    /// `(discipline, status)` with status `covered`, `stale` or `missing`.
    pub disciplines: Vec<(String, String)>,
    pub experiments: Vec<ExperimentRow>,
}

/// One workload from the manifests directory.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorkloadRow {
    pub name: String,
    pub namespace: String,
    pub kind: String,
    pub labels: BTreeMap<String, String>,
}

/// An active suppression of one risk.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SuppressionRow {
    pub actor: String,
    pub reason: String,
    pub expires_at_ns: Option<i64>,
}

/// One risk from the scanner, open or suppressed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RiskRow {
    pub target: Option<String>,
    pub environment: Option<String>,
    pub subject: Option<String>,
    pub severity: String,
    pub recommended_kind: Option<String>,
    pub suppression: Option<SuppressionRow>,
}

/// One registered experiment definition.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DefinitionRow {
    /// Registry id; `None` when the definition is known only by name.
    pub id: Option<String>,
    pub name: String,
}

/// One run (or manual record) inside the window.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunRow {
    pub id: String,
    /// Experiment name.
    pub experiment: String,
    pub system: Option<String>,
    pub environment: Option<String>,
    pub started_ns: i64,
    pub failed: bool,
    pub manual: bool,
}

/// What a `covers` edge starts from.
#[derive(Debug, Clone, PartialEq)]
pub enum CoverFrom {
    /// An experiment, by name.
    Experiment(String),
    /// A manual record, by id.
    Manual(String),
}

/// Evidence toward, or a declared mapping to, a compliance article.
#[derive(Debug, Clone, PartialEq)]
pub struct CoverRow {
    pub from: CoverFrom,
    /// `ChaosGraph` article id, for example `compliance:DORA/Art.25`.
    pub article: String,
    /// `true` when backed by a run (`evidences`), `false` when only
    /// declared (`maps_to_compliance`).
    pub evidenced: bool,
}

/// One compliance article node.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ArticleRow {
    pub id: String,
    pub label: String,
}

/// A declared service dependency from a topology file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DependsRow {
    pub from: String,
    pub to: String,
}

/// A `ChaosGraph` coverage gap (an available fault that was never injected).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GapRow {
    /// `gap:<plugin>::<action>`.
    pub id: String,
    pub label: String,
}

/// Everything [`super::assemble`] needs. Row order carries no meaning
/// except in the two glob lists, where the first match wins.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StarInputs {
    pub as_of_ns: i64,
    pub org_units: Vec<OrgUnitRow>,
    pub entity_globs: Vec<GlobRow>,
    pub experiment_globs: Vec<GlobRow>,
    pub services: Vec<ServiceRow>,
    pub calls: Vec<CallRow>,
    pub systems: Vec<SystemRow>,
    pub workloads: Vec<WorkloadRow>,
    pub risks: Vec<RiskRow>,
    pub definitions: Vec<DefinitionRow>,
    pub runs: Vec<RunRow>,
    pub covers: Vec<CoverRow>,
    pub articles: Vec<ArticleRow>,
    pub depends: Vec<DependsRow>,
    pub gaps: Vec<GapRow>,
}

/// Depth used when `focus` is set and `depth` is not.
pub const DEFAULT_DEPTH: u8 = 2;
/// Largest `depth` a request may ask for.
pub const MAX_DEPTH: u8 = 4;
/// Default node budget before clustering starts.
pub const DEFAULT_BUDGET: usize = 1500;
/// Largest node budget a request may ask for.
pub const MAX_BUDGET: usize = 5000;

/// The request, already validated by the handler.
#[derive(Debug, Clone, PartialEq)]
pub struct StarQuery {
    pub range: String,
    /// The requested environment, echoed in the response.
    pub env: Option<String>,
    /// Environments the caller may see. `None` means every environment,
    /// including nodes that carry none.
    pub scope: Option<Vec<String>>,
    pub focus: Option<String>,
    /// BFS depth around `focus`; defaults to [`DEFAULT_DEPTH`].
    pub depth: Option<u8>,
    pub kinds: Option<Vec<super::StarKind>>,
    pub budget: usize,
    pub include_runs: bool,
}

impl Default for StarQuery {
    fn default() -> Self {
        Self {
            range: "7d".to_string(),
            env: None,
            scope: None,
            focus: None,
            depth: None,
            kinds: None,
            budget: DEFAULT_BUDGET,
            include_runs: false,
        }
    }
}
