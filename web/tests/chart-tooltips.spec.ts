import { test } from '@playwright/test';
import type { Page } from '@playwright/test';

import { EVIL, expectShownAsText, installChartHooks, showTip } from './charts';

// One test per page with a chart: a name that arrives from the lake with
// markup in it is shown in the tooltip as text. The DORA heatmap and the
// star map have theirs next to their other tests.

const me = {
    auth_required: false,
    authenticated: true,
    username: null,
    role: null,
    env_scopes: [],
};

const NOW_MS = Date.UTC(2026, 10, 14, 12);
const NOW_NS = NOW_MS * 1_000_000;
const HOUR_NS = 3_600_000_000_000;

async function mock(page: Page, bodies: Record<string, unknown>) {
    await installChartHooks(page);
    await page.route('**/api/**', async (route) => {
        const path = new URL(route.request().url()).pathname;
        const body = path === '/api/me' ? me : bodies[path];
        // What a test did not give is not there: the page says so or goes without.
        if (body === undefined) await route.fulfill({ status: 404, json: { error: 'not found' } });
        else await route.fulfill({ json: body });
    });
}

test('overview: a fault type with markup is text in the tooltip', async ({ page }) => {
    await mock(page, {
        '/api/overview': {
            range: '24h',
            from_ns: NOW_NS - 24 * HOUR_NS,
            to_ns: NOW_NS,
            kpis: [],
            experiments_per_day: [{ ts: NOW_MS / 1000, v: 3 }],
            targets: [{ target: EVIL, experiments: 3, pass_rate: 1 }],
            faults: [{ fault_type: EVIL, fault_subtype: EVIL, count: 3 }],
        },
        '/api/experiments/windows': {
            count: 1,
            runs: [
                {
                    id: 'run-1',
                    name: EVIL,
                    start_ns: NOW_NS - HOUR_NS,
                    end_ns: NOW_NS,
                    outcome: 'completed',
                },
            ],
        },
    });
    await page.goto('/');
    const faults = await showTip(page, 'faults', { seriesIndex: 0, dataIndex: 0 });
    await expectShownAsText(page, faults, `${EVIL} / ${EVIL}`);
    // The bars carry the band of a run, named after it on the canvas.
    const day = await showTip(page, 'per-day', { seriesIndex: 0, dataIndex: 0 });
    await expectShownAsText(page, day, '3');
});

test('logs: a severity with markup is text in the volume tooltip', async ({ page }) => {
    await mock(page, {
        '/api/logs': { count: 0, logs: [] },
        '/api/logs/volume': {
            interval: '1h',
            bucket_s: 3600,
            rows: [
                { ts: NOW_MS / 1000 - 3600, severity: EVIL, count: 4 },
                { ts: NOW_MS / 1000, severity: EVIL, count: 2 },
            ],
        },
    });
    await page.goto('/logs');
    const tip = await showTip(page, 'log-volume', { seriesIndex: 0, dataIndex: 0 });
    await expectShownAsText(page, tip);
});

test('metrics: a metric and a group with markup are text in the tooltip', async ({ page }) => {
    const points = [
        { ts: NOW_MS / 1000 - 3600, v: 1 },
        { ts: NOW_MS / 1000, v: 2 },
    ];
    await mock(page, {
        '/api/metrics/catalog': { metrics: [{ name: EVIL, types: ['gauge'], dimensions: [EVIL] }] },
        '/api/metrics/query': {
            name: EVIL,
            type: 'gauge',
            interval: '1h',
            range: '24h',
            group_by: EVIL,
            series: [{ group: EVIL, points }],
        },
        '/api/experiments/windows': { count: 0, runs: [] },
    });
    await page.goto('/metrics');
    const tip = await showTip(page, 'metric', { seriesIndex: 0, dataIndex: 0 });
    await expectShownAsText(page, tip);
});

test('traces: a trace id with markup is text in the duration tooltip', async ({ page }) => {
    await mock(page, {
        '/api/traces': { count: 0, traces: [] },
        '/api/traces/durations': {
            points: [{ trace_id: EVIL, ts_ns: NOW_NS - HOUR_NS, duration_ms: 120 }],
            p50_ms: 120,
            p95_ms: 120,
            p99_ms: 120,
        },
    });
    await page.goto('/traces');
    const tip = await showTip(page, 'trace-durations', { seriesIndex: 0, dataIndex: 0 });
    await expectShownAsText(page, tip);
    await expectShownAsText(page, tip, '120ms');
});

test('scores: a unit with markup in its path and band is text in the treemap tooltip', async ({
    page,
}) => {
    const unit = {
        path: EVIL,
        name: EVIL,
        kind: 'team',
        score: 62.5,
        band: EVIL,
        coverage: 0.5,
        scored: 1,
        expected: 2,
        weakest: null,
        weight: 1,
        children: [],
    };
    await mock(page, {
        '/api/scores/tree': {
            ...unit,
            path: '',
            name: 'Company',
            band: 'fair',
            children: [unit],
            delta: 0,
            sparkline: [],
        },
    });
    await page.goto('/scores');
    // The first item of a treemap is its root; the unit is the one after.
    const tip = await showTip(page, 'score-treemap', { seriesIndex: 0, dataIndex: 1 });
    await expectShownAsText(page, tip);
    await expectShownAsText(page, tip, `score 62.5 (${EVIL})`);
    await expectShownAsText(page, tip, 'coverage 1/2');
});

test('posture: targets, disciplines and objectives with markup are text in both heatmaps', async ({
    page,
}) => {
    const coverage = {
        kinds: [EVIL],
        targets: [
            {
                target: EVIL,
                environment: 'demo',
                criticality: null,
                cells: [{ kind: EVIL, status: 'stale', runs: 1, last_run_ns: 1 }],
            },
        ],
        portfolio: [{ kind: EVIL, covered: 0, stale: 1, missing: 0, ratio: 0 }],
    };
    const summary = { total: 0, critical: 0, high: 0, medium: 0, low: 0, suppressed: 0 };
    const nfr = {
        target: EVIL,
        environment: 'demo',
        experiment: EVIL,
        nfr_id: EVIL,
        kind: 'latency',
        state: EVIL,
        criticality: 'major',
        threshold: 250,
        observed: 180,
        margin: 70,
    };
    await mock(page, {
        '/api/posture': {
            as_of_ns: 1,
            posture: { score: 50, band: 'fair', targets: [] },
            summary,
            coverage,
            top_risks: [],
            nfrs: [nfr],
        },
        '/api/risks': { as_of_ns: 1, risks: [], suppressed: [], summary, coverage },
    });
    await page.goto('/posture');
    const covered = await showTip(page, 'coverage', { seriesIndex: 0, cell: [0, 0] });
    await expectShownAsText(page, covered, `${EVIL} · ${EVIL}`);
    await expectShownAsText(page, covered, 'stale');
    const objective = await showTip(page, 'nfr', { seriesIndex: 0, cell: [0, 0] });
    await expectShownAsText(page, objective, `${EVIL} · ${EVIL} (${EVIL})`);
    await expectShownAsText(page, objective, `${EVIL} · observed 180 vs 250 · margin 70`);
});
