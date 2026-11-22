import { expect, test } from '@playwright/test';
import type { Page } from '@playwright/test';

import { installChartHooks } from './charts';

// The charts of the Overview, Logs, Metrics, Traces, Scores, Resilience
// and Performance pages draw their grid lines, axes, borders, lines and
// tooltips in the colours of the theme in force, and take the colours of
// a new theme when it changes. The fan chart is the score trend of the
// Resilience page.

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
const DAY_S = 86_400;

const overview = {
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

const bodies: Record<string, unknown> = {
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

type Theme = 'dark' | 'light';

/** Open the pages in `theme`, the way a saved choice of the user does. */
async function mockApi(page: Page, theme: Theme, answers: Record<string, unknown> = bodies) {
    await installChartHooks(page);
    await page.addInitScript((saved) => localStorage.setItem('tumult.theme', saved), theme);
    await page.route('**/api/**', async (route) => {
        const body = answers[new URL(route.request().url()).pathname];
        if (body === undefined) await route.fulfill({ status: 404, json: { error: 'not found' } });
        else await route.fulfill({ json: body });
    });
}

/** The colours of the tokens of each theme (lib/theme.css). */
const COLOURS = {
    dark: {
        grid: '#242d36',
        axis: '#33404c',
        text: '#8b98a5',
        tooltip: '#151b21',
        panel: '#151b21',
        bg: '#0b0e11',
        scale: ['#1c242c', '#2d5d84', '#5eb1ef'],
        accent: '#5eb1ef',
        status: ['#ef4444', '#f59e0b', '#34d399'],
    },
    light: {
        grid: '#e3e8ed',
        axis: '#c5ced6',
        text: '#5b6772',
        tooltip: '#ffffff',
        panel: '#ffffff',
        bg: '#f4f6f8',
        scale: ['#e9eef2', '#9cc4e4', '#1f6fb2'],
        accent: '#1f6fb2',
        status: ['#c0392b', '#b45309', '#0f8a5f'],
    },
};

/** What a chart draws its frame in. A key is absent where the chart has no such part. */
type Frame = {
    grid?: string;
    axis?: string;
    label?: string;
    legend?: string;
    border?: string;
    calendar?: string;
    scale?: string[];
    tooltip?: string;
    line?: string;
    mark?: string;
    markLabel?: string;
};

/** The colours of the frame of chart `id` as the page drew it, or null before it is drawn. */
const frameOf = (page: Page, id: string) =>
    page.evaluate((chart): Frame | null => {
        interface Colour {
            color?: string;
        }
        interface Axis {
            axisLine?: { lineStyle?: Colour };
            axisLabel?: Colour;
            splitLine?: { lineStyle?: Colour };
            axisName?: Colour;
        }
        interface Option {
            xAxis?: Axis[];
            yAxis?: Axis[];
            radar?: Axis[];
            legend?: { textStyle?: Colour }[];
            series?: {
                itemStyle?: { borderColor?: string };
                lineStyle?: Colour;
                markLine?: { lineStyle?: Colour; label?: Colour };
            }[];
            calendar?: { itemStyle?: { borderColor?: string } }[];
            visualMap?: { inRange?: { color?: string[] } }[];
            tooltip?: { backgroundColor?: string }[];
        }
        const charts = (window as Window & { __tumultCharts?: Record<string, unknown> })
            .__tumultCharts;
        const o = charts?.[chart] as Option | undefined;
        if (!o?.series?.length) return null;
        // A radar chart has its grid and axes on the radar, not on x and y.
        const radar = o.radar?.[0];
        const marked = o.series.find((s) => s.markLine)?.markLine;
        const frame: Frame = {
            grid: (o.yAxis?.[0] ?? radar)?.splitLine?.lineStyle?.color,
            axis: (o.xAxis?.[0] ?? radar)?.axisLine?.lineStyle?.color,
            label: o.xAxis?.[0]?.axisLabel?.color ?? radar?.axisName?.color,
            legend: o.legend?.[0]?.textStyle?.color,
            border: o.series[0]?.itemStyle?.borderColor,
            calendar: o.calendar?.[0]?.itemStyle?.borderColor,
            scale: o.visualMap?.[0]?.inRange?.color,
            tooltip: o.tooltip?.[0]?.backgroundColor,
            line: o.series.find((s) => s.lineStyle?.color)?.lineStyle?.color,
            mark: marked?.lineStyle?.color,
            markLabel: marked?.label?.color,
        };
        return Object.fromEntries(Object.entries(frame).filter(([, v]) => v !== undefined));
    }, id);

/** The frame each chart is drawn in, in the colours of `theme`. */
function framesIn(theme: Theme): Record<string, Frame> {
    const c = COLOURS[theme];
    const axes = { grid: c.grid, axis: c.axis, label: c.text, tooltip: c.tooltip };
    return {
        'per-day': axes,
        faults: { legend: c.text, border: c.panel, tooltip: c.tooltip },
        'log-volume': { ...axes, legend: c.text },
        metric: { ...axes, legend: c.text },
        'trace-durations': axes,
        'score-treemap': { border: c.bg, tooltip: c.tooltip },
        coverage: { ...axes, scale: c.status },
        'coverage-radar': { ...axes, line: c.accent },
        nfr: { ...axes, scale: c.status },
        trend: { ...axes, line: c.text },
        perf: { ...axes, legend: c.text, mark: c.axis, markLabel: c.text },
    };
}

async function expectFrame(page: Page, id: string, theme: Theme) {
    await expect
        .poll(() => frameOf(page, id), { message: `chart ${id} is drawn in the ${theme} theme` })
        .toMatchObject(framesIn(theme)[id] ?? {});
}

// The coverage heatmap and radar and the objectives of the Resilience page.
const RESILIENCE = ['coverage', 'coverage-radar', 'nfr'];

const PAGES: { path: string; charts: string[] }[] = [
    { path: '/', charts: ['per-day', 'faults'] },
    { path: '/logs', charts: ['log-volume'] },
    { path: '/metrics?name=requests', charts: ['metric'] },
    { path: '/traces', charts: ['trace-durations'] },
    { path: '/scores', charts: ['score-treemap'] },
    { path: '/resilience', charts: RESILIENCE },
    { path: '/performance?target=checkout&env=demo', charts: ['perf'] },
];

for (const theme of ['dark', 'light'] as const) {
    for (const { path, charts } of PAGES) {
        test(`the charts of ${path} are drawn in the colours of the ${theme} theme`, async ({
            page,
        }) => {
            await mockApi(page, theme);
            await page.goto(path);
            await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
            for (const id of charts) await expectFrame(page, id, theme);
        });
    }

    test(`the fan chart of the score trend is drawn in the colours of the ${theme} theme`, async ({
        page,
    }) => {
        await mockApi(page, theme);
        await page.goto('/resilience');
        await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
        await expectFrame(page, 'trend', theme);
    });

    test(`the calendar of the overview is drawn in the colours of the ${theme} theme`, async ({
        page,
    }) => {
        const c = COLOURS[theme];
        await mockApi(page, theme, {
            ...bodies,
            '/api/overview': {
                ...overview,
                range: '7d',
                from_ns: NOW_NS - 7 * 24 * HOUR_NS,
                experiments_per_day: [
                    { ts: NOW_MS / 1000 - DAY_S, v: 4 },
                    { ts: NOW_MS / 1000, v: 2 },
                ],
            },
        });
        await page.goto('/?range=7d');
        await expect
            .poll(() => frameOf(page, 'per-day'))
            .toMatchObject({ calendar: c.axis, scale: c.scale, tooltip: c.tooltip });
    });
}

/** Open `path` in the dark theme, then pick the light theme with the theme button. */
async function expectPicked(page: Page, path: string, charts: string[]) {
    await mockApi(page, 'dark');
    await page.goto(path);
    for (const id of charts) await expectFrame(page, id, 'dark');

    // The theme button goes from dark to light.
    await page.locator('button.theme').click();
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');
    for (const id of charts) await expectFrame(page, id, 'light');
}

const PICKED: { name: string; path: string; charts: string[] }[] = [
    { name: 'the overview', path: '/', charts: ['per-day', 'faults'] },
    { name: 'the Resilience page', path: '/resilience', charts: RESILIENCE },
    {
        name: 'the Performance page',
        path: '/performance?target=checkout&env=demo',
        charts: ['perf'],
    },
];

for (const { name, path, charts } of PICKED) {
    test(`the charts of ${name} take the colours of the theme the user picks`, ({ page }) =>
        expectPicked(page, path, charts));
}

test('the fan chart of the score trend takes the colours of the theme the user picks', ({ page }) =>
    expectPicked(page, '/resilience', ['trend']));
