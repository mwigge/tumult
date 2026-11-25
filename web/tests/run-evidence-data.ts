// The page of a run with all of its evidence: steady state, steps,
// objectives, load and the samples of its probes, as the API gives them.
// The run evidence spec and the style snapshot read it.

import { EVIL } from './charts';

export const S = 1_000_000_000;
export const T0 = 1_794_852_855 * S;

export const experiment = {
    id: 'run-1',
    name: 'checkout-saturation',
    started_ns: T0,
    duration_ns: 120 * S,
    trace_id: null,
    target_system: 'checkout',
    target_technology: 'http',
    target_environment: 'staging',
    status: 'deviated',
    deviations: null,
    duration_ms: '120000',
    journal_only: true,
    faults: null,
    origin: 'automated',
    review_status: null,
};

export function step(
    phase: string,
    name: string,
    second: number,
    more: Record<string, unknown> = {},
) {
    return {
        phase,
        name,
        activity_type: 'probe',
        status: 'succeeded',
        started_at_ns: T0 + second * S,
        duration_ms: 6,
        output: null,
        error: null,
        ...more,
    };
}

function sample(probe: string, phase: string, second: number, ms: number, ok = true) {
    return { probe, phase, ts_ns: T0 + second * S, duration_ms: ms, ok };
}

function loadWindow(second: number, rps: number, p95: number, errorRate: number) {
    return {
        ts_ns: T0 + second * S,
        window_s: 10,
        throughput_rps: rps,
        latency_p50_ms: 20,
        latency_p95_ms: p95,
        latency_p99_ms: p95 + 20,
        error_rate: errorRate,
    };
}

export const load = {
    tool: 'oha',
    started_at_ns: T0,
    ended_at_ns: T0 + 30 * S,
    duration_s: 30,
    vus: 400,
    throughput_rps: 2466.93,
    latency_p50_ms: 20,
    latency_p95_ms: 80,
    latency_p99_ms: 100.06,
    error_rate: 0.0125,
    total_requests: 302618,
    thresholds_met: false,
    windows: [
        loadWindow(0, 2400, 70, 0),
        loadWindow(10, 2500, 95, 0.025),
        loadWindow(20, 2450, 80, 0),
    ],
};

export const FULL = {
    kind: 'robustness',
    started_at_ns: T0,
    ended_at_ns: T0 + 120 * S,
    hypothesis: { before_met: true, after_met: false },
    recovery: {
        rto_target_s: 30,
        rto_measured_s: 12.5,
        rpo_target_s: 5,
        rpo_measured_s: null,
    },
    steps: [
        step('hypothesis_before', 'health', 0, { output: 'ok' }),
        step('method', 'cpu-stress', 8.26, { activity_type: 'action', duration_ms: 122_663 }),
        step('method', 'process-still-up', 9, {
            status: 'failed',
            duration_ms: 40,
            error: 'the process is gone',
        }),
        step('hypothesis_after', 'health', 118, { status: 'timeout', duration_ms: 5000 }),
        step('rollback', 'heal', 119, { activity_type: 'action', output: 'healed' }),
    ],
    objectives: [
        {
            nfr_id: 'fast-enough',
            kind: 'latency',
            metric: 'load.latency_p95_ms',
            operator: 'lte',
            threshold: 200,
            observed: 80,
            state: 'met',
            margin: 120,
            criticality: 'blocking',
        },
        {
            nfr_id: 'many-requests',
            kind: 'throughput',
            metric: 'load.total_requests',
            operator: 'gte',
            threshold: 400000,
            observed: 302618,
            state: 'breached',
            margin: -97382,
            criticality: 'blocking',
        },
        {
            nfr_id: 'rpo',
            kind: 'rpo',
            metric: 'rpo',
            operator: 'lte',
            threshold: 5,
            observed: null,
            state: 'inconclusive',
            margin: null,
            criticality: 'advisory',
        },
    ],
    load,
    probe_samples: [
        sample('health', 'baseline', 1, 6),
        sample(EVIL, 'baseline', 1, 9),
        sample('health', 'during', 10, 20),
        sample('health', 'during', 20, 97, false),
        sample('health', 'post', 119, 5),
    ],
    probe_samples_total: 5,
};
