// The API answers of the pages that draw charts: Overview, Logs, Metrics,
// Traces, Scores, Resilience and Performance, keyed by path. The chart
// theme spec and the style snapshot read them.

const me = {
    auth_required: false,
    authenticated: true,
    username: null,
    role: null,
    env_scopes: [],
};

export const NOW_MS = Date.UTC(2026, 10, 14, 12);
export const NOW_NS = NOW_MS * 1_000_000;
export const HOUR_NS = 3_600_000_000_000;
export const DAY_S = 86_400;

export const overview = {
    range: '24h',
    from_ns: NOW_NS - 24 * HOUR_NS,
    to_ns: NOW_NS,
    kpis: [],
    experiments_per_day: [{ ts: NOW_MS / 1000, v: 3 }],
    targets: [],
    faults: [
        { fault_type: 'injection', fault_subtype: 'tumult-network', count: 2 },
        { fault_type: 'injection', fault_subtype: 'tumult-process', count: 1 },
    ],
};

const unit = {
    path: 'commerce',
    name: 'commerce',
    kind: 'team',
    score: 62.5,
    band: 'fair',
    coverage: 0.5,
    scored: 1,
    expected: 2,
    weakest: null,
    weight: 1,
    children: [],
};

// One target of the Resilience page, tested for chaos and not for
// performance.
const coverage = {
    kinds: ['chaos', 'performance'],
    targets: [
        {
            target: 'database',
            environment: 'demo',
            criticality: null,
            cells: [
                { kind: 'chaos', status: 'covered', runs: 2, last_run_ns: 1 },
                { kind: 'performance', status: 'missing', runs: 0, last_run_ns: null },
            ],
        },
    ],
    portfolio: [
        { kind: 'chaos', covered: 1, stale: 0, missing: 0, ratio: 1 },
        { kind: 'performance', covered: 0, stale: 0, missing: 1, ratio: 0 },
    ],
};

const summary = { total: 0, critical: 0, high: 0, medium: 0, low: 0, suppressed: 0 };

const nfr = (nfr_id: string, state: string, margin: number) => ({
    target: 'database',
    environment: 'demo',
    experiment: 'pg-failover',
    nfr_id,
    kind: 'latency',
    state,
    criticality: 'major',
    threshold: 250,
    observed: 250 - margin,
    margin,
});

// One version of the target of the Performance page.
const side = (version: string, p50: number, p95: number, p99: number) => ({
    version,
    runs: [`run-${version}`],
    samples: 24,
    dropped: 0,
    p50,
    p95,
    p99,
    attribution: { declared: 0, observed: 1, inferred: 0 },
    run_details: [
        {
            experiment_id: `run-${version}`,
            attribution: 'observed',
            status: 'completed',
            truncated: false,
            aborted: false,
            included: true,
        },
    ],
    partial: { truncated: 0, aborted: 0, halted: 0, deviated: 0, excluded: 0 },
    windows: {
        planned: 24,
        completed: 24,
        unanswered: 0,
        lost: 0,
        unanswered_share: 0,
        censored: 0,
    },
    requests: { sent: 2400, failed: 0, failure_share: 0, runs_without_counts: 0 },
    distribution: Array.from({ length: 21 }, (_, i) => ({
        p: i * 5,
        value: p50 + ((p99 - p50) * i) / 20,
    })),
});

const version = (name: string, seen: number) => ({
    version: name,
    first_seen_ns: seen,
    last_seen_ns: 2,
    last_deployed_ns: seen,
    redeployed: false,
});

const perf = {
    target: 'checkout',
    probe: 'load.latency_p95_ms',
    environment: 'demo',
    environments_seen: ['demo'],
    range: '30d',
    since_ns: NOW_NS - 30 * 24 * HOUR_NS,
    selection: 'latest_deployment',
    selection_rule: 'in demo, 1.1.0 was deployed last and 1.0.0 before it',
    backfill: 'done',
    include_partial: false,
    verdict: 'regressed',
    reason: 'latency',
    reason_codes: ['latency'],
    reasons: ['p95 difference interval lies above the margin'],
    baseline: side('1.0.0', 8, 9.3, 10),
    candidate: side('1.1.0', 41, 47.5, 49),
    comparison: {
        quantile: 'p95',
        delta: 38.2,
        delta_pct: 412,
        ci_low: 36.9,
        ci_high: 39.4,
        confidence: 0.95,
        margin: 0.5,
        iterations: 2000,
        seed: 7,
        min_samples: 20,
        min_runs: 3,
        low_run_count: true,
        failure_share: { baseline: 0, candidate: 0, delta: 0, margin: 0.01 },
    },
    versions_seen: [version('1.1.0', 1), version('1.0.0', 0)],
    unattributed_runs: { count: 0, reasons: {}, runs: [] },
    runs_on_other_versions: 0,
    stored: false,
    duplicate: false,
    history: [],
    as_of_ns: NOW_NS,
};

export const bodies: Record<string, unknown> = {
    '/api/me': me,
    '/api/overview': overview,
    '/api/experiments/windows': { count: 0, runs: [] },
    '/api/logs': { count: 0, logs: [] },
    '/api/logs/volume': {
        interval: '1h',
        bucket_s: 3600,
        rows: [
            { ts: NOW_MS / 1000 - 3600, severity: 'ERROR', count: 4 },
            { ts: NOW_MS / 1000, severity: 'INFO', count: 2 },
        ],
    },
    '/api/metrics/catalog': { metrics: [{ name: 'requests', types: ['gauge'], dimensions: [] }] },
    '/api/metrics/query': {
        name: 'requests',
        type: 'gauge',
        interval: '1h',
        range: '24h',
        group_by: null,
        series: [
            {
                group: null,
                points: [
                    { ts: NOW_MS / 1000 - 3600, v: 1 },
                    { ts: NOW_MS / 1000, v: 2 },
                ],
            },
        ],
    },
    '/api/traces': { count: 0, traces: [] },
    '/api/traces/durations': {
        points: [{ trace_id: 'trace-1', ts_ns: NOW_NS - HOUR_NS, duration_ms: 120 }],
        p50_ms: 120,
        p95_ms: 120,
        p99_ms: 120,
    },
    '/api/scores/tree': {
        ...unit,
        path: '',
        name: 'Company',
        children: [unit],
        delta: 0,
        sparkline: [
            [1, 58],
            [2, 60],
            [3, 62.5],
        ],
    },
    '/api/posture': {
        as_of_ns: NOW_NS,
        posture: { score: 62.5, band: 'fair', targets: [] },
        summary,
        coverage,
        top_risks: [],
        nfrs: [nfr('p99_latency', 'met', 70), nfr('p50_latency', 'breached', -20)],
    },
    '/api/risks': { as_of_ns: NOW_NS, risks: [], suppressed: [], summary, coverage },
    '/api/perf': {
        targets: [{ target: 'checkout', environments: ['demo'], runs: 2, last_run_ns: NOW_NS }],
        range: '30d',
        as_of_ns: NOW_NS,
    },
    '/api/perf/checkout': perf,
};
