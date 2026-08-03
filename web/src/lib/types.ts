// Shared types mirroring the query API JSON shapes.

export interface SparkPoint {
  ts: number;
  v: number;
}

export interface Kpi {
  name: string;
  label: string;
  unit: 'count' | 'ratio' | 'seconds';
  value: number | null;
  delta: number | null;
  spark: SparkPoint[];
}

export interface Overview {
  range: string;
  from_ns: number;
  to_ns: number;
  kpis: Kpi[];
  experiments_per_day: SparkPoint[];
  targets: { target: string; experiments: number; pass_rate: number | null }[];
  faults: { fault_type: string; fault_subtype: string | null; count: number }[];
}

export interface ExperimentRow {
  id: string;
  name: string | null;
  started_ns: number;
  duration_ns: number | null;
  trace_id: string | null;
  target_system: string | null;
  target_technology: string | null;
  target_environment: string | null;
  status: string | null;
  deviations: string | null;
  duration_ms: string | null;
  faults: string | null;
  origin: 'automated' | 'manual' | null;
  review_status: string | null;
}

/** One experiment run's time window, from `GET /api/experiments/windows`. */
export interface ExperimentWindow {
  id: string | null;
  name: string | null;
  start_ns: number;
  end_ns: number;
  outcome: string | null;
}

export interface Span {
  ts_ns: number;
  trace_id: string;
  span_id: string;
  parent_span_id: string | null;
  span_name: string;
  span_kind: string;
  duration_ns: number;
  status_code: string;
  status_message: string;
  service_name: string;
  fault_type: string | null;
  fault_subtype: string | null;
  // Present on trace-detail rows (experiment roots carry these in tumult).
  experiment_id?: string | null;
  experiment_name?: string | null;
  span_attrs: Record<string, string>;
  events: unknown;
}

export interface LogRow {
  ts_ns: number;
  severity_text: string | null;
  body: string;
  trace_id: string | null;
  span_id: string | null;
  log_attrs: Record<string, string>;
}

export interface MetricPoint {
  kind: 'sum' | 'gauge';
  ts_ns: number;
  metric_name: string;
  value: number;
  outcome_status: string | null;
  plugin_name: string | null;
}

export interface ExperimentDetail {
  experiment: ExperimentRow;
  spans: Span[];
  logs: LogRow[];
  metrics: MetricPoint[];
}

export interface Dimensions {
  outcomes: string[];
  targets: string[];
  faults: string[];
  experiments: string[];
}

export interface MetricDefInfo {
  name: string;
  description: string | null;
}

export interface Timeseries {
  metric: string;
  description: string | null;
  interval: string;
  range: string;
  points: { bucket_s: number; value: number | null }[];
}

export interface AskResponse {
  configured: boolean;
  source?: 'golden' | 'llm';
  sql?: string;
  rows?: Record<string, unknown>[];
  error?: string;
}

export interface ReportFile {
  name: string;
  bytes: number;
  modified_s: number;
}

export interface LogEntry {
  ts_ns: number;
  severity_text: string | null;
  body: string;
  trace_id: string | null;
  span_id: string | null;
  service_name: string | null;
  experiment_id: string | null;
  log_attrs: Record<string, string>;
  resource_attrs: Record<string, string>;
}

export interface LogVolume {
  interval: string;
  bucket_s: number;
  rows: { ts: number; severity: string; count: number }[];
}

export interface TraceRow {
  trace_id: string;
  started_ns: number;
  duration_ns: number;
  span_count: number;
  error_count: number;
  root_name: string | null;
  service_name: string | null;
  experiment_id: string | null;
  experiment_name: string | null;
  status: string | null;
}

export interface TraceDurations {
  points: { trace_id: string; ts_ns: number; duration_ms: number }[];
  p50_ms: number | null;
  p95_ms: number | null;
  p99_ms: number | null;
}

export interface TraceDetail {
  trace_id: string;
  spans: Span[];
  logs: LogRow[];
}

export interface MetricCatalogEntry {
  name: string;
  types: ('sum' | 'gauge' | 'histogram')[];
  dimensions: string[];
}

export interface MetricSeries {
  group: string | null;
  points: { ts: number; v?: number | null; avg?: number | null; p95?: number | null }[];
}

export interface MetricQueryResult {
  name: string;
  type: 'sum' | 'gauge' | 'histogram';
  interval: string;
  range: string;
  group_by: string | null;
  series: MetricSeries[];
}

export interface TopologyNode {
  id: string;
  name: string;
  type: 'service' | 'target';
  runs: number;
  errors: number;
  avg_duration_ns: number | null;
}

export interface TopologyEdge {
  from_id: string;
  to_id: string;
  weight: number;
}

export interface Topology {
  nodes: TopologyNode[];
  edges: TopologyEdge[];
}

// ---------------------------------------------------------------------------
// v2: resilience scoring + compliance reports

export type RunState = 'passed' | 'stale' | 'failed' | 'never_run';

export interface ExperimentScore {
  name: string;
  target: string | null;
  score: number;
  state: RunState;
  band: string;
  last_run_ns: number | null;
  last_outcome: string | null;
  runs: number;
}

export interface TargetScore {
  target: string;
  score: number;
  band: string;
  runs: number;
  last_run_ns: number | null;
}

export interface Scorecard {
  portfolio: number;
  band: string;
  delta: number | null;
  as_of_ns: number;
  targets: TargetScore[];
  experiments: ExperimentScore[];
}

export type ReportTemplate = 'executive-digest' | 'game-day' | 'evidence-pack';

export interface ReportMetaV2 {
  doc_id: string;
  type: ReportTemplate;
  title: string;
  created_ns: number;
  data_as_of_ns: number;
  bytes: number;
  sha256: string;
  params: {
    period: string | null;
    experiment_id: string | null;
    framework: string | null;
  };
}

// ---------------------------------------------------------------------------
// v0.5: org hierarchy rollups + manual evidence

export type RunStateV5 = 'passed' | 'stale' | 'partial' | 'failed' | 'never_run';

export interface OrgNodeScore {
  path: string;
  name: string;
  kind: string;
  score: number;
  band: string;
  coverage: number;
  scored: number;
  expected: number;
  weakest: string | null;
  weight: number;
  children: OrgNodeScore[];
}

export interface ScoreTree extends OrgNodeScore {
  delta: number;
  sparkline: [number, number][];
}

export interface ManualExperiment {
  id: string;
  experiment_name: string;
  exercise_type: string;
  executed_at_ns: number;
  hypothesis: string;
  method: string;
  outcome_status: string;
  hypothesis_met: boolean | null;
  findings: string | null;
  action_items: unknown;
  target_system: string | null;
  target_environment: string | null;
  blast_radius: string | null;
  recovery_time_s: number | null;
  duration_s: number | null;
  origin: string;
  entered_by: string;
  entered_at_ns: number;
  attestation: string;
  status: 'draft' | 'submitted' | 'verified' | 'rejected';
  reviewed_by: string | null;
  reviewed_at_ns: number | null;
  review_note: string | null;
  renewal_due_ns: number | null;
  framework_refs: string[] | null;
  batch_id: string | null;
  content_hash: string;
}

export interface ManualAuditRow {
  id: string;
  experiment_id: string;
  changed_by: string;
  changed_at_ns: number;
  action: string;
  diff: unknown;
  prev_hash: string | null;
  new_hash: string;
}

export interface EvidenceAttachment {
  id: string;
  experiment_id: string;
  kind: string;
  uri: string;
  label: string | null;
  file_hash: string | null;
  added_by: string;
  added_at_ns: number;
}

export interface ManualDetail {
  experiment: ManualExperiment;
  audit: ManualAuditRow[];
  attachments: EvidenceAttachment[];
}

export interface ManualRecordInput {
  experiment_name: string;
  exercise_type: string;
  executed_at_ns: number;
  hypothesis: string;
  method: string;
  outcome_status: string;
  hypothesis_met?: boolean | null;
  findings?: string | null;
  action_items?: string[];
  target_system?: string | null;
  target_environment?: string | null;
  blast_radius?: string | null;
  recovery_time_s?: number | null;
  duration_s?: number | null;
  entered_by: string;
  attestation: string;
  renewal_due_ns?: number | null;
  framework_refs?: string[];
}

// --- auth (tumultd session API) ---------------------------------------------

export type Role = 'viewer' | 'operator' | 'approver' | 'admin';

/** `POST /api/auth/login` 200 body. */
export interface LoginResponse {
  username: string;
  role: Role;
  must_change: boolean;
}

/**
 * `GET /api/me` — always 200. `auth_required: false` means the daemon has no
 * users (open local mode); the UI then behaves as if auth did not exist.
 */
export interface MeResponse {
  auth_required: boolean;
  authenticated: boolean;
  username?: string;
  role?: Role;
  must_change?: boolean;
  env_scopes?: string[];
}

// --- admin: user management (tumultd admin API, Admin role) -------------------

/** `GET /api/users` row — never carries the password hash. */
export interface AdminUser {
  id: string;
  username: string;
  role: Role;
  must_change: boolean;
  disabled: boolean;
  created_at_ns: number;
  env_scopes: string[];
}

/** `POST /api/users` 201 body — `one_time_password` only when no password was supplied. */
export interface CreateUserResponse {
  id: string;
  username: string;
  role: Role;
  must_change: boolean;
  one_time_password?: string;
}

// --- UI execution: run registry + runs (tumultd run-control API) ------------

/**
 * `runs.state` values. Active (can still transition): queued / validating /
 * running / stopping / pending_approval (T10); everything else is terminal,
 * including T10's `rejected` (quorum refused) and `expired` (approval TTL
 * lapsed). (`RunState` above is already taken by the score-freshness enum.)
 */
export type RunExecState =
  | 'queued'
  | 'validating'
  | 'running'
  | 'stopping'
  | 'pending_approval'
  | 'passed'
  | 'deviated'
  | 'failed'
  | 'aborted'
  | 'orphaned'
  | 'rollback_pending'
  | 'rejected'
  | 'expired';

/** `GET /api/registry` row. */
export interface RegistryEntry {
  id: string;
  name: string;
  content_hash: string;
  registered_at_ns: number;
  registered_by: string | null;
}

/** `GET /api/registry/{id}` — one definition including the TOON source. */
export interface RegistryDefinition extends RegistryEntry {
  definition_toon: string;
}

/** One method/rollback/probe step in a dry-run plan. `timeout_s` lives on
    the provider for most provider types; both spots are tolerated. */
export interface DryRunStep {
  name: string;
  activity_type: string;
  provider: { type?: string; timeout_s?: number | null; [key: string]: unknown };
  timeout_s?: number | null;
  [key: string]: unknown;
}

/** `POST /api/runs/dry-run` plan (valid:true). Only the fields the UI
    renders are typed in detail; the rest ride along. */
export interface DryRunPlan {
  title: string;
  description: string;
  tags: string[];
  estimate: {
    expected_outcome?: string;
    expected_recovery_s?: number | null;
    confidence?: string | null;
    rationale?: string | null;
  } | null;
  baseline: unknown;
  hypothesis: { title: string; probes: DryRunStep[] } | null;
  guards: unknown;
  method: DryRunStep[];
  rollbacks: DryRunStep[];
  controls: unknown;
  regulatory: unknown;
  blast_radius: unknown;
}

export type DryRunResponse =
  | { valid: true; registry_id: string; plan: DryRunPlan }
  | { valid: false; error: string };

/** `GET /api/runs` / `GET /api/runs/{id}` run row. */
export interface RunRow {
  id: string;
  registry_id: string;
  state: RunExecState;
  params_json: string | null;
  experiment_id: string | null;
  rollback_status: string | null;
  error: string | null;
  queued_at_ns: number;
  started_at_ns: number | null;
  ended_at_ns: number | null;
  definition_name: string | null;
}

/** `GET /api/runs/{id}` audit entry (oldest first). */
export interface RunAuditEntry {
  run_id: string;
  at_ns: number;
  event: string;
  detail: string | null;
  actor: string | null;
}

// --- T10: approval workflow --------------------------------------------------

export type ApprovalTier = 'T1' | 'T2' | 'T3';

/**
 * Approval request for a gated run — the shape embedded in
 * `GET /api/runs/{id}` (`approval.request`) and returned per row by
 * `GET /api/approvals`.
 */
export interface ApprovalRequest {
  run_id: string;
  state: string;
  queued_at_ns: number;
  params_json: string | null;
  definition_name: string | null;
  tier: ApprovalTier;
  /** SHA-256 pin of the approved definition bytes (64 hex chars). */
  pin_hash: string;
  env: string;
  target: string | null;
  quorum_required: number;
  requested_by: string;
  requested_at_ns: number;
  expires_at_ns: number;
  /** Null while pending; set once the request was consumed (run dispatched). */
  consumed_at_ns: number | null;
  break_glass: boolean;
  break_glass_by: string | null;
  break_glass_justification: string | null;
  approved_count: number;
}

/** `GET /api/approvals` queue row — same shape as the embedded request. */
export type ApprovalQueueRow = ApprovalRequest;

/** One approver decision, oldest first in `approval.decisions`. */
export interface ApprovalDecision {
  run_id: string;
  approver: string;
  decision: 'approved' | 'rejected';
  note: string | null;
  decided_at_ns: number;
}

export interface RunDetail {
  run: RunRow;
  audit: RunAuditEntry[];
  /** T10: always present; `request` is null when the run never gated. */
  approval: {
    request: ApprovalRequest | null;
    decisions: ApprovalDecision[];
  };
}
