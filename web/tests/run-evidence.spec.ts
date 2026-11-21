import { expect, test } from '@playwright/test';
import type { Locator, Page } from '@playwright/test';

import { EVIL, expectShownAsText, installChartHooks, showTip } from './charts';

// The page of a run shows the evidence of that run: its steady state, its
// steps, its objectives, its load and the samples of its probes. The API
// is mocked here; `live-tests` reads the same page from a real daemon.

const me = {
    auth_required: false,
    authenticated: true,
    username: null,
    role: null,
    env_scopes: [],
};

const S = 1_000_000_000;
const T0 = 1_794_852_855 * S;

const experiment = {
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

function step(phase: string, name: string, second: number, more: Record<string, unknown> = {}) {
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

const load = {
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

const FULL = {
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

const NOTHING = {
    ...FULL,
    kind: 'chaos',
    hypothesis: { before_met: null, after_met: null },
    recovery: {
        rto_target_s: null,
        rto_measured_s: null,
        rpo_target_s: null,
        rpo_measured_s: null,
    },
    steps: [step('method', 'act', 0, { activity_type: 'action' })],
    objectives: [],
    load: null,
    probe_samples: [],
    probe_samples_total: 0,
};

async function open(page: Page, evidence: unknown) {
    await installChartHooks(page);
    await page.route('**/api/**', async (route) => {
        const path = new URL(route.request().url()).pathname;
        if (path === '/api/me') await route.fulfill({ json: me });
        else if (path === '/api/experiments/run-1')
            await route.fulfill({
                json: { experiment, spans: [], logs: [], metrics: [], evidence },
            });
        else await route.fulfill({ status: 404, json: { error: 'not found' } });
    });
    await page.goto('/experiments/run-1');
    await expect(page.locator('.page-head')).toContainText('checkout-saturation');
}

// The text of every cell of a table, row by row.
async function cells(table: Locator, rows: number): Promise<string[][]> {
    await expect(table.locator('tbody tr')).toHaveCount(rows);
    return table
        .locator('tbody tr')
        .evaluateAll((all) =>
            all.map((row) =>
                [...row.querySelectorAll('td')].map((cell) =>
                    (cell.textContent ?? '').replace(/\s+/g, ' ').trim(),
                ),
            ),
        );
}

interface Drawn {
    series: { name: string; data: unknown[]; markArea?: unknown }[];
}

// The option the chart `id` was drawn with.
async function drawn(page: Page, id: string): Promise<Drawn> {
    const read = (chart: string): Drawn => {
        const w = window as Window & { __tumultCharts?: Record<string, unknown> };
        const option = w.__tumultCharts?.[chart] as Drawn | undefined;
        const series = option?.series ?? [];
        return {
            series: series.map(({ name, data, markArea }) => ({ name, data, markArea })),
        };
    };
    await expect
        .poll(async () => (await page.evaluate(read, id)).series.length, {
            message: `chart ${id} is drawn`,
        })
        .toBeGreaterThan(0);
    return page.evaluate(read, id);
}

test('the page says what the steady state and the recovery of the run were', async ({ page }) => {
    await open(page, FULL);

    const steady = page.getByTestId('evidence-steady');
    await expect(steady.getByTestId('steady-before')).toHaveText('met');
    await expect(steady.getByTestId('steady-before')).toHaveClass(/\bok\b/);
    await expect(steady.getByTestId('steady-after')).toHaveText('not met');
    await expect(steady.getByTestId('steady-after')).toHaveClass(/\bfail\b/);
    await expect(steady.getByTestId('run-kind')).toHaveText('robustness');
    expect(await cells(steady.getByRole('table', { name: 'Recovery' }), 2)).toEqual([
        ['RTO', '30 s', '12.5 s'],
        ['RPO', '5 s', 'not measured'],
    ]);
    await expect(page.getByTestId('no-evidence')).toHaveCount(0);
});

test('the page lists the steps of the run in the order it took them', async ({ page }) => {
    await open(page, FULL);

    const steps = page.getByTestId('evidence-steps');
    await expect(steps.getByRole('heading')).toHaveText('Steps (5)');
    expect(await cells(steps.getByRole('table'), 5)).toEqual([
        ['Steady state before', 'health', 'probe', 'succeeded', '+0.0 s', '6ms', 'ok'],
        ['Method', 'cpu-stress', 'action', 'succeeded', '+8.3 s', '2m 3s', ''],
        ['Method', 'process-still-up', 'probe', 'failed', '+9.0 s', '40ms', 'the process is gone'],
        ['Steady state after', 'health', 'probe', 'timeout', '+118.0 s', '5.0s', ''],
        ['Rollback', 'heal', 'action', 'succeeded', '+119.0 s', '6ms', 'healed'],
    ]);
    const badges = steps.locator('tbody .badge');
    await expect(badges.nth(0)).toHaveClass(/\bok\b/);
    await expect(badges.nth(2)).toHaveClass(/\bfail\b/);
    await expect(badges.nth(3)).toHaveClass(/\bfail\b/);
});

test('the page lists the objectives of the run with their verdicts', async ({ page }) => {
    await open(page, FULL);

    const objectives = page.getByTestId('evidence-objectives');
    await expect(objectives.getByRole('heading')).toHaveText('Objectives (3)');
    expect(await cells(objectives.getByRole('table'), 3)).toEqual([
        ['fast-enough', 'load.latency_p95_ms', '≤ 200', '80', '+120', 'met', 'blocking'],
        [
            'many-requests',
            'load.total_requests',
            '≥ 400000',
            '302618',
            '-97382',
            'breached',
            'blocking',
        ],
        ['rpo', 'rpo', '≤ 5', '—', '—', 'inconclusive', 'advisory'],
    ]);
    const badges = objectives.locator('tbody .badge');
    await expect(badges.nth(0)).toHaveClass(/\bok\b/);
    await expect(badges.nth(1)).toHaveClass(/\bfail\b/);
    await expect(badges.nth(2)).toHaveClass(/\bwarn\b/);
});

test('the page shows the totals of the load and draws its windows', async ({ page }) => {
    await open(page, FULL);

    const panel = page.getByTestId('evidence-load');
    const facts = await panel
        .locator('dl > div')
        .evaluateAll((all) =>
            all.map((fact) => [
                fact.querySelector('dt')?.textContent?.trim(),
                fact.querySelector('dd')?.textContent?.trim(),
            ]),
        );
    expect(facts).toEqual([
        ['Tool', 'oha'],
        ['Virtual users', '400'],
        ['Requests', '302618'],
        ['Throughput', '2466.9 requests/s'],
        ['Latency p50', '20.0 ms'],
        ['Latency p95', '80.0 ms'],
        ['Latency p99', '100.1 ms'],
        ['Error rate', '1.25 %'],
        ['Thresholds', 'not met'],
    ]);

    await expect(
        panel.getByRole('img', { name: 'Latency of the load over time: p50, p95 and p99' }),
    ).toBeVisible();
    await expect(
        panel.getByRole('img', { name: 'Throughput and error rate of the load over time' }),
    ).toBeVisible();
    const latency = await drawn(page, 'run-load-latency');
    expect(latency.series.map((series) => series.name)).toEqual(['p50', 'p95', 'p99']);
    expect(latency.series[1].data).toEqual([
        [0, 70],
        [10, 95],
        [20, 80],
    ]);
    const throughput = await drawn(page, 'run-load-throughput');
    expect(throughput.series.map((series) => [series.name, series.data])).toEqual([
        [
            'throughput',
            [
                [0, 2400],
                [10, 2500],
                [20, 2450],
            ],
        ],
        [
            'error rate',
            [
                [0, 0],
                [10, 2.5],
                [20, 0],
            ],
        ],
    ]);
});

test('the page draws the samples of each probe and marks the one out of tolerance', async ({
    page,
}) => {
    await open(page, FULL);

    const panel = page.getByTestId('evidence-samples');
    await expect(panel.getByRole('heading')).toHaveText('Probe samples (5)');
    await expect(panel.getByTestId('samples-note')).toHaveCount(0);
    const chart = await drawn(page, 'run-probe-samples');
    expect(chart.series.map((series) => series.name)).toEqual(['health', EVIL]);
    expect(chart.series[0].data).toHaveLength(4);
    expect(chart.series[0].data[2]).toMatchObject({ value: [20, 97], symbolSize: 9 });
    expect(chart.series[0].markArea).toMatchObject({
        data: [[{ name: 'during the fault', xAxis: 10 }, { xAxis: 20 }]],
    });

    const tip = await showTip(page, 'run-probe-samples', { seriesIndex: 1, dataIndex: 0 });
    await expectShownAsText(page, tip);
    await expect(tip).toContainText('9 ms');
});

test('the charts of the run follow the theme', async ({ page }) => {
    await open(page, FULL);
    // The sample out of tolerance is drawn in the colour of a failure.
    const colour = async () => {
        const chart = await drawn(page, 'run-probe-samples');
        return (chart.series[0].data[2] as { itemStyle: { color: string } }).itemStyle.color;
    };
    expect(await colour()).toBe('#ef4444');

    const toggle = page.locator('button.theme');
    for (let i = 0; i < 3; i += 1) {
        if ((await page.locator('html').getAttribute('data-theme')) === 'light') break;
        await toggle.click();
    }
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');
    await expect.poll(colour).toBe('#c0392b');
});

test('the charts of the run are drawn in the colours of the light theme on a direct load', async ({
    page,
}) => {
    await page.addInitScript(() => localStorage.setItem('tumult.theme', 'light'));
    await open(page, FULL);
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');
    const colours = () =>
        page.evaluate(() => {
            type Option = {
                tooltip?: { backgroundColor?: string }[];
                series?: { data?: { itemStyle?: { color?: string } }[] }[];
            };
            const w = window as Window & { __tumultCharts?: Record<string, unknown> };
            const of = (id: string) => w.__tumultCharts?.[id] as Option | undefined;
            return {
                latency: of('run-load-latency')?.tooltip?.[0]?.backgroundColor,
                throughput: of('run-load-throughput')?.tooltip?.[0]?.backgroundColor,
                samples: of('run-probe-samples')?.tooltip?.[0]?.backgroundColor,
                // The sample out of tolerance is drawn in the colour of a failure.
                failed: of('run-probe-samples')?.series?.[0]?.data?.[2]?.itemStyle?.color,
            };
        });
    await expect.poll(colours).toEqual({
        latency: '#ffffff',
        throughput: '#ffffff',
        samples: '#ffffff',
        failed: '#c0392b',
    });
});

test('a run without a steady state, an objective, a load or a sample says so', async ({ page }) => {
    await open(page, NOTHING);

    await expect(page.getByTestId('evidence-steady')).toContainText(
        'This run checked no steady state.',
    );
    await expect(page.getByTestId('steady-before')).toHaveCount(0);
    await expect(page.getByTestId('evidence-steady').getByRole('table')).toHaveCount(0);
    await expect(page.getByTestId('run-kind')).toHaveText('chaos');
    await expect(page.getByTestId('evidence-objectives')).toContainText(
        'This run had no objective.',
    );
    await expect(page.getByTestId('evidence-load')).toContainText('This run had no load.');
    await expect(page.getByTestId('evidence-samples')).toContainText(
        'This run took no probe samples.',
    );
    await expect(page.locator('[data-chart]')).toHaveCount(0);
    expect(await cells(page.getByTestId('evidence-steps').getByRole('table'), 1)).toEqual([
        ['Method', 'act', 'action', 'succeeded', '+0.0 s', '6ms', ''],
    ]);
});

test('a run the lake has no journal of says that it has no evidence', async ({ page }) => {
    await open(page, null);

    await expect(page.getByTestId('no-evidence')).toContainText(
        'The lake has no journal of this run',
    );
    for (const panel of ['steady', 'steps', 'objectives', 'load', 'samples']) {
        await expect(page.getByTestId(`evidence-${panel}`)).toHaveCount(0);
    }
});

test('a run with more samples than the page carries says how many it has', async ({ page }) => {
    await open(page, { ...FULL, probe_samples_total: 5003 });

    const panel = page.getByTestId('evidence-samples');
    await expect(panel.getByRole('heading')).toHaveText('Probe samples (5003)');
    await expect(panel.getByTestId('samples-note')).toHaveText(
        'The chart shows the first 5 of the 5003 samples of this run.',
    );
});

test('a load of one window has its totals and no chart', async ({ page }) => {
    await open(page, { ...FULL, load: { ...load, windows: load.windows.slice(0, 1) } });

    const panel = page.getByTestId('evidence-load');
    await expect(panel.locator('dl > div')).toHaveCount(9);
    await expect(panel.locator('[data-chart]')).toHaveCount(0);
    await drawn(page, 'run-probe-samples');
});
