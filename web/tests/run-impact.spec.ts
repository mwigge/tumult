import { expect, test } from '@playwright/test';

// The API is mocked; the fan chart renders to canvas, so it is asserted
// through the option mirrored at window.__tumultCharts.

const me = {
    auth_required: false,
    authenticated: true,
    username: null,
    role: null,
    env_scopes: [],
};

const run = {
    run: {
        id: 'run-1',
        registry_id: 'reg-1',
        state: 'deviated',
        params_json: null,
        experiment_id: 'exp-1',
        rollback_status: null,
        error: null,
        queued_at_ns: 1,
        started_at_ns: 1,
        ended_at_ns: 2,
        definition_name: 'pg-failover',
    },
    audit: [],
    approval: { request: null, decisions: [] },
};

const T0 = 1_700_000_000_000;
const ts = Array.from({ length: 8 }, (_, i) => T0 + i * 5_000);
const provenance = {
    model: 'holt-winters',
    version: '2.22.0',
    weights_hash: 'none',
    fallback_reason: null,
};

const impact = {
    run_id: 'run-1',
    experiment_id: 'exp-1',
    as_of_ns: 1,
    stable_steps: 3,
    recovery_time_s: 10,
    impact_peak: 4.2,
    provenance,
    probes: [
        {
            probe: 'p95-latency',
            reference: 'post_backcast',
            step_ns: 5_000_000_000,
            fault: { start_ns: ts[0] * 1e6, end_ns: ts[3] * 1e6 },
            series: {
                ts,
                observed: [40, 52, 47, 14, 11, 10, 10, 11],
                median: [10, 10, 10, 10, 10, 10, 10, 10],
                bands: [
                    {
                        level: 0.8,
                        lower: [8, 8, 8, 9, 9, 9, 9, 9],
                        upper: [12, 12, 12, 11, 11, 11, 11, 11],
                    },
                ],
            },
            impact: {
                per_sample: [15, 21, 18.5, 4, 1, 0, 0, 1],
                during_mean: 3.9,
                during_peak: 4.2,
                during_peak_ts_ms: ts[1],
                during_outside_ratio: 1,
                post_mean: 0.4,
                samples_during: 3,
                samples_post: 5,
            },
            recovery: {
                recovered_at_ns: ts[5] * 1e6,
                recovery_time_s: 10,
                stable_steps: 3,
                outside_during_fault: 3,
                outside_after_fault: 1,
            },
            forecast: { context_points: 5, insufficient_history: true },
            provenance,
        },
        { probe: 'error-rate', error: 'no during-phase samples' },
    ],
};

async function mockApi(page: import('@playwright/test').Page, withImpact: boolean) {
    await page.route('**/api/**', async (route) => {
        const path = new URL(route.request().url()).pathname;
        let body: unknown = {};
        let status = 200;
        if (path === '/api/me') body = me;
        else if (path === '/api/runs/run-1') body = run;
        else if (path === '/api/runs/run-1/impact') {
            if (withImpact) body = impact;
            else {
                status = 404;
                body = { error: 'run has no probe samples' };
            }
        } else if (path === '/api/experiments/exp-1')
            body = { experiment: null, spans: [], logs: [], metrics: [] };
        else if (path.endsWith('/dry-run')) body = { valid: false, errors: [] };
        await route.fulfill({
            status,
            contentType: 'application/json',
            body: JSON.stringify(body),
        });
    });
}

test('run page shows the counterfactual impact with band, fault window and provenance', async ({
    page,
}) => {
    await mockApi(page, true);
    await page.goto('/runs/run-1');

    const panel = page.getByTestId('impact');
    await expect(panel).toContainText('p95-latency');
    await expect(page.getByTestId('impact-summary')).toContainText('4.2×');
    await expect(page.getByTestId('impact-summary')).toContainText('10 s');
    await expect(page.getByTestId('impact-provenance')).toContainText('holt-winters');
    await expect(page.getByTestId('impact-provenance')).toContainText('weights none');
    await expect(panel).toContainText('backcast from the post-fault samples');
    await expect(panel).toContainText('short history (5 points)');
    // A probe that could not be compared says why instead of drawing a chart.
    await expect(panel).toContainText('no during-phase samples');

    await expect
        .poll(() =>
            page.evaluate(() => {
                const w = window as Window & { __tumultCharts?: Record<string, unknown> };
                return w.__tumultCharts?.['impact-0'] !== undefined;
            }),
        )
        .toBe(true);
    const chart = await page.evaluate(() => {
        type Series = {
            name?: string;
            data?: [number, number | null][];
            markArea?: { data?: [{ name?: string; xAxis?: number }, { xAxis?: number }][] };
            markLine?: { data?: { name?: string; xAxis?: number }[] };
        };
        const w = window as Window & { __tumultCharts?: Record<string, unknown> };
        const opt = w.__tumultCharts?.['impact-0'] as { series?: Series[] } | undefined;
        const series = opt?.series ?? [];
        const observed = series.find((s) => s.name === 'observed');
        const forecast = series.find((s) => s.name === 'forecast');
        const band = series.find((s) => s.name === '80 % band');
        const area = observed?.markArea?.data?.[0];
        return {
            observed: observed?.data?.map((d) => d[1]),
            median: forecast?.data?.map((d) => d[1]),
            bandWidth: band?.data?.map((d) => d[1]),
            fault: area ? [area[0].name, area[0].xAxis, area[1].xAxis] : null,
            marks: observed?.markLine?.data?.map((m) => [m.name, m.xAxis]),
        };
    });
    expect(chart.observed).toEqual([40, 52, 47, 14, 11, 10, 10, 11]);
    expect(chart.median).toEqual([10, 10, 10, 10, 10, 10, 10, 10]);
    expect(chart.bandWidth).toEqual([4, 4, 4, 2, 2, 2, 2, 2]);
    expect(chart.fault).toEqual(['fault', ts[0], ts[3]]);
    expect(chart.marks).toEqual([
        ['peak', ts[1]],
        ['recovered', ts[5]],
    ]);
});

test('run without probe samples shows a note instead of a chart', async ({ page }) => {
    await mockApi(page, false);
    await page.goto('/runs/run-1');
    await expect(page.getByTestId('impact')).toContainText('No probe samples to compare');
});
