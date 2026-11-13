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

/** `GET /api/gate/{target}`: the CI gate verdict for one target. */
export interface GateVerdict {
    target: string;
    score: number;
    band: string;
    verdict: 'pass' | 'warn' | 'fail';
    reasons: string[];
    thresholds: { min_score: number; max_age_days: number };
    as_of_ns: number;
    evidence: {
        runs: number;
        last_run_id: string | null;
        last_run_at_ns: number | null;
        last_outcome: string | null;
        experiments: ExperimentScore[];
    };
}

/** One side of `GET /api/perf/{target}`: a `service.version` and its samples. */
export interface PerfSide {
    version: string;
    runs: string[];
    samples: number;
    dropped: number;
    p50: number | null;
    p95: number | null;
    p99: number | null;
}

/** `GET /api/perf/{target}`: the regression verdict between the latest two versions. */
export interface PerfVerdict {
    target: string;
    probe: string;
    verdict: 'improved' | 'unchanged' | 'regressed' | 'inconclusive';
    reasons: string[];
    baseline: PerfSide | null;
    candidate: PerfSide;
    comparison: {
        quantile: 'p50' | 'p95' | 'p99';
        delta: number | null;
        delta_pct: number | null;
        ci_low: number | null;
        ci_high: number | null;
        confidence: number;
        margin: number;
        iterations: number;
        seed: number;
        min_samples: number;
    };
    versions_seen: { version: string; first_seen_ns: number; last_seen_ns: number }[];
    stored: boolean;
    history: Record<string, unknown>[];
    as_of_ns: number;
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

/** `GET /api/tokens` row — never carries the token hash. */
export interface ApiToken {
    id: string;
    user_id: string;
    username: string | null;
    name: string;
    created_at_ns: number;
    last_used_at_ns: number | null;
    revoked: boolean;
    expires_at_ns: number | null;
}

/** `POST /api/tokens` 201 body — `token` is the plaintext, returned exactly once. */
export interface CreateTokenResponse {
    id: string;
    token: string;
    expires_at_ns: number | null;
}

/** `GET /api/webhooks` row — never carries the HMAC secret. */
export interface Webhook {
    id: string;
    name: string;
    url: string;
    events: string[];
    enabled: boolean;
    created_by: string | null;
    created_at_ns: number;
}

/** `POST /api/webhooks` 201 body — `secret` is returned exactly once. */
export interface CreateWebhookResponse extends Webhook {
    secret: string;
}

/** `GET /api/events` row — one cross-run audit event (hash-chained). */
export interface RunEvent {
    run_id: string;
    at_ns: number;
    event: string;
    detail: string | null;
    actor: string | null;
    prev_hash: string | null;
    new_hash: string | null;
    definition_name: string | null;
}

/** `GET /api/gamedays` row — campaign metadata. */
export interface GameDayEntry {
    id: string;
    name: string;
    content_hash: string;
    registered_at_ns: number;
    registered_by: string | null;
}

/** `GET /api/gamedays/{id}` — the parsed campaign plan. */
export interface GameDayDetail {
    id: string;
    title: string;
    description: string | null;
    tags: string[];
    regulatory: {
        frameworks: string[];
        requirements: { id: string; description: string; evidence: string }[];
    } | null;
    scoring: { pass_threshold: number; mttr_target_s: number; recovery_required: boolean };
    experiments: {
        path: string;
        compliance_maps: string[];
        registry_id: string;
        name: string | null;
    }[];
    registered_at_ns: number;
    registered_by: string | null;
}

/** `POST /api/gamedays/validate` response. */
export interface ValidateGameDayResponse {
    valid: boolean;
    gameday_registry_id: string;
    experiments: { path: string; registry_id: string }[];
}

/** `GET /api/schedules` row (plus the joined definition name). */
export interface Schedule {
    id: string;
    name: string;
    registry_id: string;
    definition_name: string | null;
    interval_s: number;
    vars_json: string | null;
    env: string;
    target: string | null;
    enabled: boolean;
    next_run_at_ns: number;
    last_run_at_ns: number | null;
    last_run_id: string | null;
    created_by: string | null;
    created_at_ns: number;
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
    blast_radius: string | null;
    scope: DryRunScope;
}

/**
 * The dry-run plan's blast-radius summary (`plan.scope`): the declared note,
 * the fault actions with their target-identifying arguments, the guards and
 * the concurrent-fault cap. Always present — nulls/empty when undeclared.
 */
export interface DryRunScope {
    blast_radius: string | null;
    actions: {
        step: string;
        provider: string;
        action: string;
        targets: Record<string, unknown>;
    }[];
    guards: { name: string; probe: string; min_breaches: number }[];
    max_concurrent_faults: number | null;
}

export type DryRunResponse =
    | {
          valid: true;
          registry_id: string;
          plan: DryRunPlan;
          execution_hash?: string;
          binding_hash?: string;
          execution_context?: {
              env: string;
              target?: string | null;
              tier?: string;
              error?: string | null;
          } | null;
      }
    | { valid: false; error: string };

/** `GET /api/runs` / `GET /api/runs/{id}` run row. */
export interface RunRow {
    id: string;
    registry_id: string;
    state: RunExecState;
    /** Campaign parent when this run is a gameday child (schema v12). */
    gameday_id?: string | null;
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

// --- Authoring: fault catalog + scaffold -------------------------------------

/** One documented argument of a catalog action. Plugin manifests declare no
    typed parameter schema, so every arg renders as a plain text input. */
export interface CatalogArg {
    name: string;
    required: boolean;
    description: string;
}

/** A single fault action or probe in the fault catalog. */
export interface CatalogAction {
    plugin: string;
    name: string;
    description: string;
    kind: 'action' | 'probe';
    args: CatalogArg[];
}

/** A fault domain and the actions grouped under it. */
export interface CatalogDomain {
    domain: string;
    label: string;
    actions: CatalogAction[];
}

/** `GET /api/authoring/catalog` response. */
export interface CatalogResponse {
    action_count: number;
    domains: CatalogDomain[];
}

/** `POST /api/authoring/scaffold` request — mirrors the MCP
    `tumult_scaffold_experiment` tool's argument schema. `plugin` is optional
    when `action` is fully qualified as `plugin::action`. */
export interface ScaffoldRequest {
    plugin?: string;
    action: string;
    args: Record<string, string>;
    target: string;
    probe_command?: string;
    probe_url?: string;
    probe_expect?: string;
    title?: string;
}

/** `POST /api/authoring/scaffold` response. `validation_error` is present
    only when `valid` is false. */
export interface ScaffoldResponse {
    action: string;
    toon: string;
    valid: boolean;
    validation_error?: string;
}

/** `POST /api/runs/validate` response. The endpoint validates AND registers
    (content-hash deduped): `registered` is false when the same TOON was
    already in the registry. */
export type ValidateToonResponse =
    | { valid: true; registry_id: string; name: string; registered: boolean }
    | { valid: false; error: string };

// --- Passive risk scan and posture (`/api/risks`, `/api/posture`) ---------

export type RiskSeverity = 'low' | 'medium' | 'high' | 'critical';

export interface RiskSuppression {
    risk_id: string;
    reason: string;
    actor: string;
    at_ns: number;
    expires_at_ns: number | null;
}

/** One finding of the passive scanner. `id` is stable across scans. */
export interface Risk {
    id: string;
    rule: string;
    severity: RiskSeverity;
    rank: number;
    title: string;
    detail: string;
    target: string | null;
    environment: string | null;
    subject: string;
    recommended_kind: string | null;
    evidence: string[];
    suppression: RiskSuppression | null;
}

export interface CoverageCell {
    kind: string;
    status: 'covered' | 'stale' | 'missing';
    runs: number;
    last_run_ns: number | null;
}

export interface TargetCoverage {
    target: string;
    environment: string | null;
    criticality: string | null;
    cells: CoverageCell[];
}

export interface KindCoverage {
    kind: string;
    covered: number;
    stale: number;
    missing: number;
    ratio: number;
}

export interface Coverage {
    kinds: string[];
    targets: TargetCoverage[];
    portfolio: KindCoverage[];
}

export interface RiskSummary {
    total: number;
    critical: number;
    high: number;
    medium: number;
    low: number;
    suppressed: number;
}

/** `GET /api/risks` */
export interface RiskReport {
    as_of_ns: number;
    risks: Risk[];
    suppressed: Risk[];
    summary: RiskSummary;
    coverage: Coverage;
}

export interface PostureComponents {
    nfr: number | null;
    coverage: number;
    freshness: number | null;
    recovery: number | null;
}

export interface TargetPosture {
    target: string;
    environment: string | null;
    criticality: string | null;
    score: number;
    band: string;
    components: PostureComponents;
}

/** One objective verdict from the latest run of an experiment, for the
    NFR heatmap (targets × objectives, hue = margin). */
export interface NfrCell {
    target: string;
    environment: string | null;
    experiment: string;
    nfr_id: string;
    kind: string;
    state: 'met' | 'breached' | 'inconclusive' | string;
    criticality: string;
    threshold: number;
    observed: number | null;
    margin: number | null;
}

/** `GET /api/posture` */
export interface PostureResponse {
    as_of_ns: number;
    posture: { score: number; band: string; targets: TargetPosture[] };
    summary: RiskSummary;
    coverage: Coverage;
    top_risks: Risk[];
    nfrs: NfrCell[];
}

/** `POST /api/risks/{id}/suppress` response: the hash-chained row. */
export interface SuppressionRow extends RiskSuppression {
    id: string;
    prev_hash: string | null;
    new_hash: string;
}

// --- OpenTelemetry naming-contract conformance (`/api/otel/conformance`) ---

export interface ConformanceKey {
    key: string;
    present: number;
    required_for: 'target' | 'spine' | 'recommended';
}

export interface ServiceConformance {
    service: string;
    spans: number;
    last_seen_ns: number | null;
    level: 'none' | 'target' | 'spine';
    missing: string[];
    keys: ConformanceKey[];
}

/** `GET /api/otel/conformance` */
export interface ConformanceReport {
    as_of_ns: number;
    range: string;
    contract: { target: string[]; spine: string[]; recommended: string[] };
    summary: {
        services: number;
        spine: number;
        target: number;
        none: number;
        malformed_entity_ids: number;
    };
    services: ServiceConformance[];
    /** The canonical form of `ict.entity.id`, as a pattern. */
    entity_id_pattern: string;
    /** Ids that do not match it, exactly as the service sent them. */
    malformed_entity_ids: MalformedEntityId[];
}

export interface MalformedEntityId {
    service: string;
    entity_id: string;
    spans: number;
    last_seen_ns: number | null;
    flagged_at_ingest: boolean;
    reason: string;
}

// --- DORA coverage and maturity (`/api/dora`, `/api/maturity`) ---

export type EntityProfile = 'full' | 'simplified' | 'microenterprise';

export interface LegalEntityRef {
    lei: string;
    name: string;
    profile: string;
}

/** Fields every score document opens with. */
export interface ScoreHeader {
    as_of_ns: number;
    registry_version: string;
    lei: string | null;
    entity_name: string | null;
    profile: string;
    profile_label: string;
    profile_flags: string[];
    legal_entities: LegalEntityRef[];
    disclaimer?: string;
}

export type CellState = 'known' | 'unknown' | 'no_events';
export type EvidenceStatus =
    'na' | 'unknown' | 'no_events' | 'none' | 'designed' | 'existing' | 'operating';
export type Standing = 'meets' | 'gap' | 'unknown' | 'no_events' | 'na';

export interface DoraRow {
    entity_id: string;
    label: string;
    tier: string | null;
}

export interface DoraFunction {
    entity_id: string;
    name: string;
    tier: string;
    registered: boolean;
    register_codes: string[];
    org_unit: string | null;
    rto_s: number | null;
    rpo_s: number | null;
    last_assessed_ns: number | null;
    last_assessed: string | null;
    coverage: number | null;
    unknown: number;
    band: string;
}

export interface DoraArticle {
    article: string;
    title: string;
    coverage: number | null;
    unknown: number;
    band: string;
    na: number;
    na_rows: number;
    status: Record<Standing, number> | null;
    sources: Record<'otel_only' | 'tested' | 'manual' | 'none' | 'unknown', number>;
}

export interface DoraCell {
    article: string;
    entity_id: string;
    coverage: number | null;
    unknown: number;
    state: CellState;
    status: EvidenceStatus;
    status_rank: number;
    gaps: number;
}

export interface DoraExpectation {
    id: string;
    article: string;
    cite: string;
    title: string;
    weight: number;
    expected: string;
    actual: EvidenceStatus;
    gap: number;
    standing: Standing;
    criticality: number;
    priority: number;
    short: string[];
    oldest_evidence_ns: number | null;
    oldest_evidence_date: string | null;
    days_until_operating: number | null;
    evidence: { id: string; source: string } | null;
    reason: string | null;
}

export interface DoraReadiness {
    functions: number;
    registered: number;
    with_rto: number;
    with_rpo: number;
    with_assessment: number;
    complete: number;
    register: {
        connected: boolean;
        reconciled_ns: number | null;
        signed_by: string | null;
        signed_at: string | null;
        rows: number;
    };
    unmapped_services: string[];
}

/** `GET /api/dora` */
export interface DoraReport extends ScoreHeader {
    by: 'entity' | 'org';
    portfolio: { coverage: number | null; unknown: number; band: string };
    status: Record<Standing, number>;
    rows: DoraRow[];
    functions: DoraFunction[];
    articles: DoraArticle[];
    cells: DoraCell[];
    expectations: DoraExpectation[];
    na: { count: number; rows: { id: string; article: string; cite: string }[] };
    readiness: DoraReadiness;
}

export type MaturityComponent = 'conformance' | 'coverage' | 'slo' | 'recovery' | 'loop';

export interface PillarState {
    value: number | null;
    state: string;
    reason: string | null;
}

export interface MaturityNode {
    id: string;
    kind: string;
    label: string;
    parent: string | null;
    index: number;
    level: number;
    target: number;
    own: number | null;
    pillars: Record<string, PillarState>;
    rollup: { mean: number | null; weight: number; children: number };
    unknown_share: number;
    unknown: boolean;
    critical: boolean;
    cap: {
        rule: string;
        level: number | null;
        node_ids: string[];
        origin: string[];
        origin_state: string | null;
        reason: string | null;
    };
    drag: number | null;
    note: string | null;
}

/** `GET /api/maturity` */
export interface MaturityReport extends ScoreHeader {
    scale: { internal: boolean; source: string; levels: string[] };
    level: number;
    index: number;
    target: number;
    headline: string;
    gate_level: number;
    capped: boolean;
    cap: {
        level: number | null;
        blocked_level?: number;
        component: string | null;
        reason: string | null;
    };
    components: Record<MaturityComponent, number | null>;
    component_states: Record<MaturityComponent, PillarState>;
    weights: Record<MaturityComponent, number>;
    gates: { level: number; passed: boolean; failing: string[] }[];
    unknown_share: number;
    history: { quarter: string; as_of_ns: number; level: number; index: number }[];
    rollup: MaturityNode | null;
    rollup_headline: string | null;
    by: 'node' | 'entity' | 'org' | null;
    nodes: MaturityNode[];
}
