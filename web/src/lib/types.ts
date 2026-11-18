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
    /** The run sent no trace: the lake holds its journal only. */
    journal_only: boolean;
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

/** A step of a run: a probe of its steady state, a step of its method or a rollback. */
export interface EvidenceStep {
    /** `hypothesis_before`, `method`, `hypothesis_after` or `rollback`. */
    phase: string;
    name: string;
    activity_type: string;
    status: string;
    started_at_ns: number;
    duration_ms: number;
    output: string | null;
    error: string | null;
}

/** The verdict of a run on one objective. */
export interface EvidenceObjective {
    nfr_id: string;
    kind: string;
    metric: string;
    /** `lte`, `gte` or `eq`. */
    operator: string;
    threshold: number;
    observed: number | null;
    /** `met`, `breached` or `inconclusive`. */
    state: string;
    margin: number | null;
    criticality: string;
}

/** One window of a load. The load tool tells the counts of some windows only. */
export interface LoadWindow {
    /** The start of the window. */
    ts_ns: number;
    window_s?: number;
    throughput_rps?: number;
    latency_p50_ms?: number;
    latency_p95_ms?: number;
    latency_p99_ms?: number;
    /** A share, 0 to 1. */
    error_rate?: number;
    requests_sent?: number;
    requests_ok?: number;
    requests_timed_out?: number;
    request_timeout_ms?: number;
}

/** The load of a run: its totals and its windows. */
export interface EvidenceLoad {
    tool: string;
    started_at_ns: number;
    ended_at_ns: number;
    duration_s: number;
    vus: number;
    throughput_rps: number;
    latency_p50_ms: number;
    latency_p95_ms: number;
    latency_p99_ms: number;
    /** A share, 0 to 1. */
    error_rate: number;
    total_requests: number;
    thresholds_met: boolean;
    windows: LoadWindow[];
}

/** One sample of a probe: how long it took and whether it was within its tolerance. */
export interface EvidenceProbeSample {
    probe: string;
    /** `baseline`, `during` or `post`. */
    phase: string;
    ts_ns: number;
    duration_ms: number;
    ok: boolean;
}

/** What the journal of a run says, as the lake holds it. */
export interface RunEvidence {
    kind: string;
    started_at_ns: number;
    ended_at_ns: number;
    /** `null`: the steady state was not checked then. */
    hypothesis: { before_met: boolean | null; after_met: boolean | null };
    recovery: {
        rto_target_s: number | null;
        rto_measured_s: number | null;
        rpo_target_s: number | null;
        rpo_measured_s: number | null;
    };
    steps: EvidenceStep[];
    objectives: EvidenceObjective[];
    load: EvidenceLoad | null;
    /** The earliest first; `probe_samples_total` says how many the run has. */
    probe_samples: EvidenceProbeSample[];
    probe_samples_total: number;
}

export interface ExperimentDetail {
    experiment: ExperimentRow;
    spans: Span[];
    logs: LogRow[];
    metrics: MetricPoint[];
    /** `null`: the lake has no journal of this run. */
    evidence: RunEvidence | null;
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

export type StarKind =
    | 'org'
    | 'entity'
    | 'system'
    | 'component'
    | 'service'
    | 'workload'
    | 'ext'
    | 'def'
    | 'run'
    | 'slo'
    | 'article'
    | 'gap'
    | 'cluster';

export type StarRel =
    | 'owns'
    | 'implements'
    | 'calls'
    | 'runs_on'
    | 'depends_on'
    | 'tested_by'
    | 'broke'
    | 'covers'
    | 'measures';

export type StarGap = 'stale' | 'missing';

export interface StarEvidence {
    runs: number;
    failed: number;
    manual: number;
    last_run_ns: number | null;
    /** Discipline to `covered`, `stale` or `missing`. */
    by_discipline: Record<string, string>;
}

export interface StarRisks {
    open: number;
    suppressed: number;
    worst: string | null;
    suppression_expires_ns: number | null;
    suppressed_by: string | null;
    suppression_reason: string | null;
    recommended: string[];
}

export interface StarNode {
    id: string;
    kind: StarKind;
    label: string;
    /** Spine parent used by the layout. */
    parent: string | null;
    env: string | null;
    tier: string | null;
    org_kind: string | null;
    criticality_weight: number;
    conformance: string | null;
    last_seen_ns: number | null;
    posture: { score: number; band: string } | null;
    evidence: StarEvidence;
    gap: StarGap | null;
    risks: StarRisks;
    /** Reserved for the SLO, DORA and maturity sources; null until they exist. */
    slo: unknown;
    slo_count: number;
    dora: unknown;
    maturity: unknown;
    conflict: boolean;
    /** `malformed_id`, `multiple_owners`, `owner_conflict`; sorted. */
    flags: string[];
    /** Counted below an entity or an org unit; null on every other kind. */
    rollup: StarRollup | null;
    declared: boolean;
    children: number;
    members: number | null;
}

export interface StarRollup {
    systems: number;
    untested_systems: number;
    stale_systems: number;
    failed: number;
    risks_open: number;
    risks_suppressed: number;
    gaps: number;
}

export interface StarEdge {
    id: string;
    from: string;
    to: string;
    rel: StarRel;
    source: 'telemetry' | 'observed' | 'evidence' | 'declared';
    weight?: number;
    errors?: number;
    avg_duration_ns?: number;
}

export interface StarCluster {
    id: string;
    label: string;
    members: number;
    worst: { gap: StarGap | null; risk: string | null };
}

export type StarSource = 'ok' | 'unavailable';

/** `GET /api/starmap` */
export interface StarMap {
    as_of_ns: number;
    range: string;
    env: string | null;
    focus: string | null;
    /** Whether `focus` named a node the caller may see; null without a focus. */
    focus_found: boolean | null;
    depth: number | null;
    disclaimer: string;
    /** A source table was longer than its row limit; the map is incomplete. */
    truncated: boolean;
    sources: Record<'slo' | 'dora' | 'maturity' | 'conformance' | 'graph', StarSource>;
    maturity: unknown;
    counts: {
        nodes: number;
        edges: number;
        pruned: number;
        clustered: number;
        risks_open: number;
        risks_suppressed: number;
        /** Risks that belong to no node on the map. */
        risks_unplaced: number;
        gaps: number;
    };
    nodes: StarNode[];
    edges: StarEdge[];
    clusters: StarCluster[];
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
    /** The latest run sent no trace: the lake holds its journal only. */
    journal_only: boolean;
}

export interface TargetScore {
    target: string;
    score: number;
    band: string;
    runs: number;
    last_run_ns: number | null;
}

export interface Scorecard {
    /** Null when nothing is on record; its band is then `unknown`. */
    portfolio: number | null;
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
    /** How each run's version is known; `inferred` comes from span timing. */
    attribution: { declared: number; observed: number; inferred: number };
    run_details: {
        experiment_id: string;
        attribution: PerfAttribution;
        status: string | null;
        truncated: boolean;
        aborted: boolean;
        included: boolean;
    }[];
    /** Counts of runs; `excluded` = left out of the samples. */
    partial: {
        truncated: number;
        aborted: number;
        halted: number;
        deviated: number;
        excluded: number;
    };
    windows: {
        planned: number;
        completed: number;
        unanswered: number;
        /** Windows skipped behind an unanswered one. */
        lost: number;
        /** Fraction of the windows the load reached (0..1). */
        unanswered_share: number | null;
        /** Windows whose sample is a request given up on at its timeout. */
        censored: number;
    };
    /** The requests of all the side's runs, the partial ones too. */
    requests: {
        sent: number;
        failed: number;
        /** `null` when no requests were counted. */
        failure_share: number | null;
        runs_without_counts: number;
    };
    /** Percentiles of the samples (`p` 0..100), empty when there are none. */
    distribution: { p: number; value: number }[];
}

export type PerfAttribution = 'declared' | 'observed' | 'inferred';
export type PerfQuantile = 'p50' | 'p95' | 'p99';
export type PerfVerdictKind = 'improved' | 'unchanged' | 'regressed' | 'inconclusive';

/** `GET /api/perf`: the targets that have load evidence in the range. */
export interface PerfTargets {
    targets: {
        target: string;
        /** `null` = telemetry that names no environment. */
        environments: (string | null)[];
        runs: number;
        last_run_ns: number;
    }[];
    range: string;
    as_of_ns: number;
}

/** One stored row of the verdict history. */
export interface PerfStoredVerdict {
    target: string;
    probe: string;
    baseline_version: string;
    candidate_version: string;
    quantile: PerfQuantile;
    baseline_n: number;
    candidate_n: number;
    baseline_p50: number | null;
    baseline_p95: number | null;
    baseline_p99: number | null;
    candidate_p50: number | null;
    candidate_p95: number | null;
    candidate_p99: number | null;
    delta: number | null;
    delta_pct: number | null;
    ci_low: number | null;
    ci_high: number | null;
    confidence: number;
    margin: number;
    verdict: PerfVerdictKind;
    reasons: string[];
    computed_at_ns: number;
    envs: string[] | null;
}

/** The query of the perf read and of the store, which takes the same one. */
export interface PerfParams {
    probe?: string;
    quantile?: PerfQuantile;
    env?: string;
    baseline?: string;
    candidate?: string;
    range?: string;
    include?: 'partial';
}

/**
 * `GET /api/perf/{target}`: the regression verdict between the target's
 * candidate and baseline versions (`versions_seen` is in that order, newest
 * first) in ONE environment. `POST /api/perf/{target}/verdicts` with the same
 * query stores it and answers the same shape with `stored: true`;
 * `duplicate` says the latest stored row already held this evidence.
 */
export interface PerfVerdict {
    target: string;
    probe: string;
    /** The one environment the answer is for; `null` = untagged. */
    environment: string | null;
    environments_seen: (string | null)[];
    range: string;
    since_ns: number;
    selection: 'latest_deployment' | 'explicit';
    /** Which rule chose the pair, in words. */
    selection_rule: string;
    /** `running` while an old lake's version history is read back. */
    backfill: 'running' | 'done';
    include_partial: boolean;
    verdict: PerfVerdictKind;
    /** The code that decided, first of `reason_codes`. */
    reason: string | null;
    reason_codes: string[];
    reasons: string[];
    baseline: PerfSide | null;
    candidate: PerfSide;
    comparison: {
        quantile: PerfQuantile;
        delta: number | null;
        delta_pct: number | null;
        ci_low: number | null;
        ci_high: number | null;
        confidence: number;
        margin: number;
        iterations: number;
        seed: number;
        min_samples: number;
        min_runs: number;
        /** The interval rests on fewer than five runs on a side. */
        low_run_count?: boolean;
        /** Shares of failed requests (0..1); `null` when a side counted none. */
        failure_share: {
            baseline: number;
            candidate: number;
            delta: number;
            margin: number;
        } | null;
    };
    versions_seen: {
        version: string;
        first_seen_ns: number;
        last_seen_ns: number;
        last_deployed_ns: number;
        /** Deployed again after a later version. */
        redeployed: boolean;
    }[];
    unattributed_runs: {
        count: number;
        reasons: Record<string, number>;
        runs: { experiment_id: string; environment: string | null; reason: string }[];
    };
    runs_on_other_versions: number;
    stored: boolean;
    duplicate: boolean;
    /** Newest first, at most 10. */
    history: PerfStoredVerdict[];
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
    /** Null when no leaf of the subtree is scored; its band is then `unknown`. */
    score: number | null;
    band: string;
    coverage: number;
    scored: number;
    expected: number;
    weakest: string | null;
    weight: number;
    children: OrgNodeScore[];
}

export interface ScoreTree extends OrgNodeScore {
    /** Null when the node has no score now or had none a period ago. */
    delta: number | null;
    /** `[instant, score]`; an instant with nothing scored has no point. */
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
    /** Whether more ids than `malformed_entity_ids_limit` were found. */
    malformed_entity_ids_truncated?: boolean;
    malformed_entity_ids_limit?: number;
}

export interface MalformedEntityId {
    service: string;
    /** As received, cut to 64 characters. */
    entity_id: string;
    /** Whether the id received was longer than what is shown. */
    truncated?: boolean;
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
    /** The request scored another profile than the registered one. */
    profile_overridden: boolean;
    registered_profile: string | null;
    /** The caller sees part of the environments only. */
    scoped: boolean;
    scope_note: string | null;
    feeds: ScoreFeed[];
    params: ScoreParams;
    snapshot: ScoreSnapshot;
    proof_days: ProofDays;
    register_state: RegisterState;
    register_reason: string | null;
}

/** A feed the score reads, with what the org file declares of it. */
export interface ScoreFeed {
    name: string;
    declared: boolean;
    connected: boolean;
    reason: string | null;
}

/** The house parameters a document was scored with. */
export interface ScoreParams {
    source: 'defaults' | 'org_file' | 'last_valid_org_file';
    /** The org file was refused and never was valid: the defaults are in force. */
    fallback: boolean;
    /** The org file was refused and the last valid one is still in force. */
    held: boolean;
    reason: string | null;
    overridden: { key: string; value: unknown; default: unknown }[];
}

/** Whether the daily job stores days, and how many it has left out. */
export interface ScoreSnapshot {
    /** No day is stored while defaults stand in for a refused org file. */
    skipping: boolean;
    /** Days left out since the daemon started. */
    skipped: number;
    reason: string | null;
}

/** Days the proof tool stored for past dates: a test setting of the daemon. */
export interface ProofDays {
    enabled: boolean;
    counted: number;
    ignored: number;
    note: string | null;
}

/** Where the function register of the org file stands in the lake. */
export type RegisterState = 'none' | 'ok' | 'stale' | 'invalid';

/** One function's cell of a requirement row, in `GET /api/dora/{article}`. */
export interface DoraEvidenceRow {
    evidence_id: string;
    source: string;
    kind: string | null;
    exercise_type: string | null;
    experiment: string | null;
    at_ns: number;
    age_days: number;
    outcome: string;
    v: number;
}

export interface DoraRequirementCell {
    entity_id: string;
    tier: string;
    state: CellState;
    status: EvidenceStatus;
    streak_days: number;
    days_until_operating: number | null;
    reason: string | null;
    evidence?: DoraEvidenceRow[];
}

export interface DoraRequirement {
    id: string;
    cite: string;
    title: string;
    window_days: number | null;
    scope: string;
    scope_basis: 'regulation' | 'house';
    status: EvidenceStatus;
    expected: string;
    reason: string | null;
    entities: DoraRequirementCell[];
}

/** `GET /api/dora/{article}` */
export interface DoraArticleReport extends ScoreHeader {
    article: string;
    title: string;
    requirements: DoraRequirement[];
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
    /** `house`: the regulation does not restrict the row this way, the house does. */
    scope_basis: 'regulation' | 'house';
    window_days: number | null;
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

/** A quarter of the history: a stored day, today, or nothing stored. */
export interface MaturityHistoryPoint {
    quarter: string;
    state: 'stored' | 'today' | 'no_data';
    /** Who stored the day: `snapshot` (the daily job) or `proof` (the proof tool). */
    source: string | null;
    as_of: string | null;
    as_of_ns: number | null;
    level: number | null;
    index: number | null;
    unknown_share: number | null;
    reason: string | null;
}

/** `GET /api/maturity` */
export interface MaturityReport extends ScoreHeader {
    scale: { internal: boolean; source: string; levels: string[] };
    /** Level and index shown: the roll-up root's. */
    level: number;
    index: number;
    /** Level and index of the pillars alone, before the roll-up. */
    flat_level: number;
    flat_index: number;
    target: number;
    headline: string;
    gate_level: number;
    capped: boolean;
    cap: {
        rule: 'none' | 'gate' | 'weakest_critical_child';
        level: number | null;
        blocked_level?: number;
        component: string | null;
        node_ids?: string[];
        origin?: string[];
        reason: string | null;
    };
    components: Record<MaturityComponent, number | null>;
    component_states: Record<MaturityComponent, PillarState>;
    weights: Record<MaturityComponent, number>;
    gates: { level: number; passed: boolean; failing: string[] }[];
    /** Unknown share of the rolled-up index, the one shown. */
    unknown_share: number;
    /** Unknown share of the pillars alone. */
    flat_unknown_share: number;
    history: MaturityHistoryPoint[];
    rollup: MaturityNode | null;
    rollup_headline: string | null;
    by: 'node' | 'entity' | 'org' | null;
    nodes: MaturityNode[];
}

/** Which model produced a forecast; `fallback_reason` is set when the
    sidecar was configured but the built-in provider answered instead. It is
    a code (`unreachable`, `timeout`, `bad_response`, `rejected`, `busy`,
    `circuit_open`); the detail is in the daemon's log. */
export interface ForecastProvenance {
    model: string;
    version: string;
    weights_hash: string;
    fallback_reason: string | null;
}

/** Chart-ready series shared by the forecast and impact endpoints. */
export interface ForecastSeries {
    /** Epoch ms. */
    ts: number[];
    observed: (number | null)[];
    median: (number | null)[];
    bands: { level: number; lower: (number | null)[]; upper: (number | null)[] }[];
}

/** `GET /api/forecast` */
export interface ForecastResponse {
    metric: string;
    description: string | null;
    target: string | null;
    env: string[] | null;
    range: string;
    horizon: number;
    step_ns: number;
    from_ns: number;
    to_ns: number;
    /** Epoch ms; use these in the browser, where a double cannot hold ns. */
    from_ms: number;
    to_ms: number;
    series: ForecastSeries;
    forecast: {
        levels: number[];
        quantiles: (number | null)[][];
        context_points: number;
        insufficient_history: boolean;
        /** The band had no width and was floored. */
        degenerate_band: boolean;
    };
    provenance: ForecastProvenance;
    cached: boolean;
}

/** One probe's counterfactual in `GET /api/runs/{id}/impact`. A probe that
    could not be compared carries only `probe` and `error`. */
export interface ProbeImpact {
    probe: string;
    error?: string;
    reference?: 'baseline' | 'post_backcast';
    step_ns?: number;
    /** The reference band had no width (a flat or exactly periodic series)
        and was floored: the multiples below are lower bounds on a guess. */
    degenerate_band?: boolean;
    /** Epoch ms. The API also sends `*_ns`, which a double cannot hold. */
    fault?: { start_ms: number; end_ms: number };
    series?: ForecastSeries;
    impact?: {
        per_sample: (number | null)[];
        during_mean: number | null;
        during_peak: number | null;
        during_peak_ts_ms: number | null;
        during_outside_ratio: number | null;
        post_mean: number | null;
        samples_during: number;
        samples_post: number;
    };
    recovery?: {
        recovered_at_ms: number | null;
        recovery_time_s: number | null;
        /** `undetermined`: not watched long enough after the fault to say. */
        status?: 'recovered' | 'unrecovered' | 'undetermined';
        stable_steps: number;
        outside_during_fault: number;
        outside_after_fault: number;
        /** Observed samples after the fault end. */
        observed_after_fault?: number;
        /** Whether the last of them was inside the band; null when none. */
        ended_inside?: boolean | null;
    };
    forecast?: {
        context_points: number;
        insufficient_history: boolean;
        degenerate_band: boolean;
    };
    provenance?: ForecastProvenance;
}

/** `GET /api/runs/{id}/impact` */
export interface RunImpact {
    run_id: string;
    experiment_id: string;
    as_of_ms: number;
    stable_steps: number;
    /**
     * The slowest probe's recovery; null when any evaluated probe never
     * recovered, or when none was watched long enough to be seen recovering.
     */
    recovery_time_s: number | null;
    /**
     * What `recovery_time_s` means: `recovered` (a time), `unrecovered` (a
     * probe never settled), `undetermined` (no probe was watched long enough
     * to say) or `not_measurable` (no probe could be evaluated at all).
     */
    recovery_status: 'recovered' | 'unrecovered' | 'undetermined' | 'not_measurable';
    /** Evaluated probes that never settled back inside their band. */
    unrecovered_probes?: number;
    /** Evaluated probes not sampled long enough after the fault to say. */
    undetermined_probes?: number;
    /** Probes whose impact could be evaluated. */
    evaluated_probes?: number;
    impact_peak: number | null;
    /** True when any probe's reference band was degenerate. */
    degenerate_band: boolean;
    provenance: ForecastProvenance | null;
    probes: ProbeImpact[];
}
