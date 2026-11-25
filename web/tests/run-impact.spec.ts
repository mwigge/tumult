import { expect, test } from '@playwright/test';

import { installChartHooks } from './charts';

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
    fallback_reason: null as string | null,
};

// The nanosecond fields are strings here on purpose: the panel must read the
// millisecond ones, which a double holds exactly.
const probe = {
    probe: 'p95-latency',
    reference: 'post_backcast',
    step_ns: 5_000_000_000,
    degenerate_band: false,
    fault: { start_ns: 'unused', end_ns: 'unused', start_ms: ts[0], end_ms: ts[3] },
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
        per_sample: [15, 21, 18.5, 4, 1, 0, 0, 1] as (number | null)[],
        during_mean: 3.9 as number | null,
        during_peak: 4.2 as number | null,
        during_peak_ts_ms: ts[1] as number | null,
        during_outside_ratio: 1 as number | null,
        post_mean: 0.4 as number | null,
        samples_during: 3,
        samples_post: 5,
    },
    recovery: {
        recovered_at_ns: 'unused' as string | null,
        recovered_at_ms: ts[5] as number | null,
        recovery_time_s: 10 as number | null,
        stable_steps: 3,
        outside_during_fault: 3,
        outside_after_fault: 1,
    },
    forecast: { context_points: 5, insufficient_history: true, degenerate_band: false },
    provenance,
};

const impact = {
    run_id: 'run-1',
    experiment_id: 'exp-1',
    as_of_ns: 1,
    as_of_ms: 0,
    stable_steps: 3,
    recovery_time_s: 10 as number | null,
    recovery_status: 'recovered',
    unrecovered_probes: 0,
    evaluated_probes: 1,
    impact_peak: 4.2 as number | null,
    degenerate_band: false,
    provenance,
    probes: [probe, { probe: 'error-rate', error: 'no during-phase samples' }] as unknown[],
};

type Answer = { status: number; body: unknown; delayMs?: number };

async function mockApi(page: import('@playwright/test').Page, answer: Answer) {
    // Charts mirror their option only when this registry is already there.
    await installChartHooks(page);
    await page.route('**/api/**', async (route) => {
        const path = new URL(route.request().url()).pathname;
        let body: unknown = {};
        let status = 200;
        if (path === '/api/me') body = me;
        else if (path === '/api/runs/run-1') body = run;
        else if (path === '/api/runs/run-1/impact') {
            if (answer.delayMs) await new Promise((done) => setTimeout(done, answer.delayMs));
            status = answer.status;
            body = answer.body;
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

async function chartOption(page: import('@playwright/test').Page) {
    await expect
        .poll(() =>
            page.evaluate(() => {
                const w = window as Window & { __tumultCharts?: Record<string, unknown> };
                return w.__tumultCharts?.['impact-0'] !== undefined;
            }),
        )
        .toBe(true);
    return page.evaluate(() => {
        type Series = {
            id?: string;
            name?: string;
            data?: [number, number | null][];
            markArea?: { data?: [{ name?: string; xAxis?: number }, { xAxis?: number }][] };
            markLine?: { data?: { name?: string; xAxis?: number }[] };
        };
        const w = window as Window & { __tumultCharts?: Record<string, unknown> };
        const opt = w.__tumultCharts?.['impact-0'] as { series?: Series[] } | undefined;
        const series = opt?.series ?? [];
        const observed = series.find((s) => s.id === 'observed');
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
}

test('markup in a probe name is shown as text in the chart tooltip', async ({ page }) => {
    const payload = `<img src=x onerror="window.__fired = 1"><b id="planted">p95</b>`;
    await mockApi(page, {
        status: 200,
        body: { ...impact, probes: [{ ...probe, probe: payload }] },
    });
    await page.goto('/runs/run-1');
    await chartOption(page);
    const chart = page.locator('[data-chart="impact-0"]');
    // The pointer reaches only what the window shows.
    await chart.scrollIntoViewIfNeeded();
    const box = await chart.boundingBox();
    if (!box) throw new Error('the chart has no box');
    // The tooltip follows the pointer along the time axis.
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.move(box.x + box.width / 2 + 4, box.y + box.height / 2);
    await expect(chart).toContainText(payload);
    await expect(chart.locator('img, #planted')).toHaveCount(0);
    expect(await page.evaluate(() => '__fired' in window)).toBe(false);
});

test('run page shows the counterfactual impact with band, fault window and provenance', async ({
    page,
}) => {
    await mockApi(page, { status: 200, body: impact });
    await page.goto('/runs/run-1');

    const panel = page.getByTestId('impact');
    await expect(panel).toContainText('p95-latency');
    await expect(page.getByTestId('impact-summary')).toContainText('4.2×');
    await expect(page.getByTestId('impact-summary')).toContainText('10 s');
    await expect(page.getByTestId('impact-provenance')).toContainText('holt-winters');
    await expect(page.getByTestId('impact-provenance')).toContainText('weights none');
    await expect(page.getByTestId('impact-provenance')).not.toContainText('fallback');
    await expect(panel).toContainText('backcast from the post-fault samples');
    await expect(panel).toContainText('short history (5 points)');
    await expect(page.getByTestId('impact-degenerate')).toHaveCount(0);
    await expect(page.getByTestId('impact-error')).toHaveCount(0);
    // A probe that could not be compared says why instead of drawing a chart.
    await expect(panel).toContainText('no during-phase samples');
    // The chart is a canvas: its name is what assistive technology gets.
    await expect(
        panel.getByRole('img', { name: /p95-latency.*observed.*counterfactual/ }),
    ).toBeVisible();

    const chart = await chartOption(page);
    expect(chart.observed).toEqual([40, 52, 47, 14, 11, 10, 10, 11]);
    expect(chart.median).toEqual([10, 10, 10, 10, 10, 10, 10, 10]);
    expect(chart.bandWidth).toEqual([4, 4, 4, 2, 2, 2, 2, 2]);
    // Read from the millisecond fields; the nanosecond ones are not numbers here.
    expect(chart.fault).toEqual(['fault', ts[0], ts[3]]);
    expect(chart.marks).toEqual([
        ['peak', ts[1]],
        ['recovered', ts[5]],
    ]);
});

test('run without probe samples shows a note instead of a chart', async ({ page }) => {
    await mockApi(page, { status: 404, body: { error: 'run has no probe samples' } });
    await page.goto('/runs/run-1');
    const panel = page.getByTestId('impact');
    await expect(panel).toContainText('No probe samples to compare');
    await expect(panel.getByRole('alert')).toHaveCount(0);
});

for (const failure of [
    { status: 500, error: 'lake query failed' },
    { status: 403, error: 'forbidden' },
]) {
    test(`a ${failure.status} from the impact endpoint is an error, not "no data"`, async ({
        page,
    }) => {
        await mockApi(page, { status: failure.status, body: { error: failure.error } });
        await page.goto('/runs/run-1');
        const panel = page.getByTestId('impact');
        const alert = panel.getByRole('alert');
        await expect(alert).toContainText('could not be loaded');
        await expect(alert).toContainText(failure.error);
        await expect(panel).not.toContainText('No probe samples to compare');
    });
}

test('the panel announces that it is loading', async ({ page }) => {
    await mockApi(page, { status: 200, body: impact, delayMs: 1_500 });
    await page.goto('/runs/run-1');
    const loading = page.getByTestId('impact').getByRole('status');
    await expect(loading).toHaveAttribute('aria-busy', 'true');
    await expect(loading).toHaveAccessibleName(/loading/i);
    await expect(page.getByTestId('impact-summary')).toContainText('4.2×');
    await expect(loading).toHaveCount(0);
});

test('a run that never settled says so and has no recovery mark', async ({ page }) => {
    const unsettled = {
        ...impact,
        recovery_time_s: null,
        recovery_status: 'unrecovered',
        unrecovered_probes: 1,
        probes: [
            {
                ...probe,
                recovery: {
                    ...probe.recovery,
                    recovered_at_ns: null,
                    recovered_at_ms: null,
                    recovery_time_s: null,
                },
            },
        ],
    };
    await mockApi(page, { status: 200, body: unsettled });
    await page.goto('/runs/run-1');
    await expect(page.getByTestId('impact-summary')).toContainText('recovery not recovered');
    await expect(page.getByTestId('impact')).toContainText('recovery not recovered');
    const chart = await chartOption(page);
    expect(chart.marks).toEqual([['peak', ts[1]]]);
});

test('one probe that never recovered makes the headline not recovered', async ({ page }) => {
    const stuck = {
        ...probe,
        probe: 'error-rate',
        recovery: {
            ...probe.recovery,
            recovered_at_ns: null,
            recovered_at_ms: null,
            recovery_time_s: null,
        },
    };
    const body = {
        ...impact,
        recovery_time_s: null,
        recovery_status: 'unrecovered',
        unrecovered_probes: 1,
        evaluated_probes: 2,
        probes: [probe, stuck],
    };
    await mockApi(page, { status: 200, body });
    await page.goto('/runs/run-1');
    const summary = page.getByTestId('impact-summary');
    await expect(summary).toContainText('recovery not recovered');
    // The probe that did settle keeps its own time; the headline does not borrow it.
    await expect(summary).not.toContainText('10 s');
    await expect(page.getByTestId('impact-unrecovered')).toContainText('1 of 2 probes');
    await expect(page.getByTestId('impact')).toContainText('recovery 10 s');
});

test('a run whose probes all recovered has no unrecovered note', async ({ page }) => {
    await mockApi(page, { status: 200, body: impact });
    await page.goto('/runs/run-1');
    await expect(page.getByTestId('impact-summary')).toContainText('10 s');
    await expect(page.getByTestId('impact-unrecovered')).toHaveCount(0);
    await expect(page.getByTestId('impact-undetermined')).toHaveCount(0);
});

// A probe that was not sampled after the fault: no recovery was seen, and
// none was ruled out either.
const unwatched = {
    ...probe,
    probe: 'error-rate',
    recovery: {
        ...probe.recovery,
        recovered_at_ns: null,
        recovered_at_ms: null,
        recovery_time_s: null,
        status: 'undetermined',
        outside_after_fault: 0,
        observed_after_fault: 0,
    },
};

test('a probe not watched after the fault is undetermined, not unrecovered', async ({ page }) => {
    const body = {
        ...impact,
        unrecovered_probes: 0,
        undetermined_probes: 1,
        evaluated_probes: 2,
        probes: [probe, unwatched],
    };
    await mockApi(page, { status: 200, body });
    await page.goto('/runs/run-1');
    const summary = page.getByTestId('impact-summary');
    // The headline is the time of the probe that was watched.
    await expect(summary).toContainText('recovery 10 s');
    await expect(summary).not.toContainText('not recovered');
    await expect(page.getByTestId('impact-undetermined')).toContainText(
        '1 of 2 probes was not sampled long enough after the fault',
    );
    await expect(page.getByTestId('impact-unrecovered')).toHaveCount(0);
    const facts = page.getByTestId('impact-probe-error-rate');
    await expect(facts).toContainText('recovery undetermined');
    await expect(facts).not.toContainText('not recovered');
});

test('a run with no probe watched after the fault has an undetermined recovery', async ({
    page,
}) => {
    const body = {
        ...impact,
        recovery_time_s: null,
        recovery_status: 'undetermined',
        unrecovered_probes: 0,
        undetermined_probes: 1,
        evaluated_probes: 1,
        probes: [unwatched],
    };
    await mockApi(page, { status: 200, body });
    await page.goto('/runs/run-1');
    const summary = page.getByTestId('impact-summary');
    await expect(summary).toContainText('recovery undetermined');
    await expect(summary).not.toContainText('not recovered');
    await expect(page.getByTestId('impact-undetermined')).toContainText('1 of 1 probe was');
});

test('an unrecovered probe outranks an undetermined one in the headline', async ({ page }) => {
    const stuck = {
        ...unwatched,
        probe: 'latency',
        recovery: { ...unwatched.recovery, status: 'unrecovered', outside_after_fault: 5 },
    };
    const body = {
        ...impact,
        recovery_time_s: null,
        recovery_status: 'unrecovered',
        unrecovered_probes: 1,
        undetermined_probes: 1,
        evaluated_probes: 2,
        probes: [stuck, unwatched],
    };
    await mockApi(page, { status: 200, body });
    await page.goto('/runs/run-1');
    await expect(page.getByTestId('impact-summary')).toContainText('recovery not recovered');
    await expect(page.getByTestId('impact-unrecovered')).toContainText('1 of 2 probes');
    await expect(page.getByTestId('impact-undetermined')).toContainText('1 of 2 probes');
    await expect(page.getByTestId('impact-probe-latency')).toContainText('recovery not recovered');
});

// No probe could be evaluated: there is no finding about recovery at all,
// which is neither "not recovered" nor "undetermined".
test('a run with no evaluable probe has a recovery that is not measurable', async ({ page }) => {
    const body = {
        ...impact,
        recovery_time_s: null,
        recovery_status: 'not_measurable',
        unrecovered_probes: 0,
        undetermined_probes: 0,
        evaluated_probes: 0,
        impact_peak: null,
        provenance: null,
        probes: [{ probe: 'error-rate', error: 'no during-phase samples' }],
    };
    await mockApi(page, { status: 200, body });
    await page.goto('/runs/run-1');
    const summary = page.getByTestId('impact-summary');
    await expect(summary).toContainText('recovery not measurable');
    await expect(summary).not.toContainText('not recovered');
    await expect(summary).not.toContainText('undetermined');
    await expect(page.getByTestId('impact-not-measurable')).toContainText(
        'none of the 1 probe could be evaluated',
    );
    await expect(page.getByTestId('impact-unrecovered')).toHaveCount(0);
    await expect(page.getByTestId('impact-undetermined')).toHaveCount(0);
    await expect(page.getByTestId('impact')).toContainText('no during-phase samples');
});

// The headline is what the daemon says it is, not what the counts suggest.
test('the headline follows the recovery status the daemon reports', async ({ page }) => {
    const body = { ...impact, recovery_time_s: null, recovery_status: 'undetermined' };
    await mockApi(page, { status: 200, body });
    await page.goto('/runs/run-1');
    const summary = page.getByTestId('impact-summary');
    await expect(summary).toContainText('recovery undetermined');
    await expect(summary).not.toContainText('not recovered');
});

test('a fallback names its reason next to the model', async ({ page }) => {
    const fallen = { ...provenance, fallback_reason: 'circuit_open' };
    const body = { ...impact, provenance: fallen, probes: [{ ...probe, provenance: fallen }] };
    await mockApi(page, { status: 200, body });
    await page.goto('/runs/run-1');
    const line = page.getByTestId('impact-provenance');
    await expect(line).toContainText('holt-winters');
    await expect(page.getByTestId('impact-fallback')).toContainText('circuit open');

    // A code this build does not know is shown as it is.
    const odd = { ...provenance, fallback_reason: 'quota_exhausted' };
    await page.unroute('**/api/**');
    await mockApi(page, { status: 200, body: { ...impact, provenance: odd } });
    await page.reload();
    await expect(page.getByTestId('impact-fallback')).toContainText('quota_exhausted');
});

test('a flat reference is flagged and a missing peak is not shown as a number', async ({
    page,
}) => {
    const flat = {
        ...impact,
        impact_peak: null,
        degenerate_band: true,
        probes: [
            {
                ...probe,
                degenerate_band: true,
                impact: { ...probe.impact, during_peak: null, during_peak_ts_ms: null },
                forecast: {
                    context_points: 40,
                    insufficient_history: false,
                    degenerate_band: true,
                },
            },
        ],
    };
    await mockApi(page, { status: 200, body: flat });
    await page.goto('/runs/run-1');
    const summary = page.getByTestId('impact-summary');
    await expect(summary).toContainText('peak impact not measurable');
    await expect(summary).not.toContainText('×');
    await expect(page.getByTestId('impact-degenerate').first()).toContainText('no width');
    await expect(page.getByTestId('impact-degenerate')).toHaveCount(2);
    const chart = await chartOption(page);
    expect(chart.marks).toEqual([['recovered', ts[5]]]);
});

test('a baseline reference is described as a forecast from before the fault', async ({ page }) => {
    const body = { ...impact, probes: [{ ...probe, reference: 'baseline' }] };
    await mockApi(page, { status: 200, body });
    await page.goto('/runs/run-1');
    const panel = page.getByTestId('impact');
    await expect(panel).toContainText('forecast from the pre-fault samples');
    await expect(panel).not.toContainText('backcast');
});
