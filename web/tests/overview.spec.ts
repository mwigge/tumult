import { expect, test } from '@playwright/test';
import type { Page } from '@playwright/test';

import { installChartHooks } from './charts';

// The 24h Overview draws its runs on day columns: a busy day must name
// them once, upright, not stack every name sideways on the bar.

const NOW_MS = Date.UTC(2026, 10, 15, 23);
const NOW_NS = NOW_MS * 1_000_000;
const HOUR_NS = 3_600_000_000_000;
const RUNS = 22;

const me = {
    auth_required: false,
    authenticated: true,
    username: null,
    role: null,
    env_scopes: [],
};

const runs = Array.from({ length: RUNS }, (_, i) => ({
    id: `run-${i}`,
    name: `exp-${i}`,
    start_ns: NOW_NS - (i + 1) * (HOUR_NS / 2),
    end_ns: NOW_NS - (i + 1) * (HOUR_NS / 2) + HOUR_NS / 4,
    outcome: 'completed',
}));

const bodies: Record<string, unknown> = {
    '/api/me': me,
    '/api/overview': {
        range: '24h',
        from_ns: NOW_NS - 24 * HOUR_NS,
        to_ns: NOW_NS,
        kpis: [],
        experiments_per_day: [{ ts: NOW_MS / 1000, v: RUNS }],
        targets: [],
        faults: [],
    },
    '/api/experiments/windows': { count: RUNS, runs },
};

interface Marks {
    position: string | undefined;
    names: string[];
}

test('overview: a busy day names its runs once and upright', async ({ page }) => {
    await installChartHooks(page);
    await page.route('**/api/**', async (route) => {
        const body = bodies[new URL(route.request().url()).pathname];
        if (body === undefined) await route.fulfill({ status: 404, json: { error: 'not found' } });
        else await route.fulfill({ json: body });
    });
    await page.goto('/');
    const marks = () =>
        page.evaluate((): Marks | null => {
            const charts = (window as Window & { __tumultCharts?: Record<string, unknown> })
                .__tumultCharts;
            const option = charts?.['per-day'] as
                | {
                      series?: {
                          markLine?: { label?: { position?: string }; data?: { name: string }[] };
                      }[];
                  }
                | undefined;
            const line = option?.series?.find((s) => s.markLine)?.markLine;
            if (!line) return null;
            return { position: line.label?.position, names: (line.data ?? []).map((d) => d.name) };
        });
    await expect.poll(marks, { message: 'the run overlay is drawn' }).not.toBeNull();
    expect(await marks()).toEqual({ position: 'end', names: [`${RUNS} runs`] });
});

// What the two charts of the home page count, and what each colour is.

type Bodies = Record<string, unknown>;

async function mockApi(page: Page, answers: Bodies) {
    await installChartHooks(page);
    await page.route('**/api/**', async (route) => {
        const body = answers[new URL(route.request().url()).pathname];
        if (body === undefined) await route.fulfill({ status: 404, json: { error: 'not found' } });
        else await route.fulfill({ json: body });
    });
}

/** The colour of each swatch of the legend of chart `id`, in order. */
const swatches = (page: Page, id: string) =>
    page
        .locator(`[data-legend="${id}"] .swatch`)
        .evaluateAll((all) =>
            all.map((el) => (el as HTMLElement).style.getPropertyValue('--swatch')),
        );

interface Option {
    yAxis?: { name?: string }[];
    visualMap?: { max: number; inRange: { color: string[] } }[];
    series?: {
        data?: { name?: string }[];
        markArea?: { data: { itemStyle?: { color: string } }[][] };
    }[];
}

/** The option of chart `id` as the page drew it, or null before it is drawn. */
const optionOf = (page: Page, id: string) =>
    page.evaluate((chart) => {
        const charts = (window as Window & { __tumultCharts?: Record<string, unknown> })
            .__tumultCharts;
        const option = charts?.[chart] as Option | undefined;
        if (!option) return null;
        // Only the parts asserted: an option also holds functions.
        return {
            yAxis: option.yAxis?.map((axis) => ({ name: axis.name })),
            visualMap: option.visualMap?.map((map) => ({ max: map.max, inRange: map.inRange })),
            series: option.series?.map((series) => ({
                data: series.data?.map((d) => ({ name: d.name })),
                markArea: series.markArea && {
                    data: series.markArea.data.map((edges) =>
                        edges.map((edge) => ({ itemStyle: edge.itemStyle })),
                    ),
                },
            })),
        } as Option;
    }, id);

const window24h = (outcome: string, i: number) => ({
    id: `run-${outcome}`,
    name: `exp-${outcome}`,
    start_ns: NOW_NS - (i + 2) * HOUR_NS,
    end_ns: NOW_NS - (i + 1) * HOUR_NS,
    outcome,
});

test('overview: the per-day chart counts runs and names the colours of its bands', async ({
    page,
}) => {
    await mockApi(page, {
        '/api/me': me,
        '/api/overview': {
            ...(bodies['/api/overview'] as object),
            experiments_per_day: [{ ts: NOW_MS / 1000, v: 3 }],
            faults: [
                { fault_type: 'injection', fault_subtype: 'tumult-network', count: 2 },
                { fault_type: 'injection', fault_subtype: 'tumult-process', count: 1 },
            ],
        },
        '/api/experiments/windows': {
            count: 3,
            runs: ['completed', 'failed', 'aborted'].map(window24h),
        },
    });
    await page.goto('/');

    await expect(page.getByRole('heading', { level: 2, name: 'Runs per day' })).toBeVisible();
    await expect(page.locator('[data-chart="per-day"]')).toHaveAttribute(
        'aria-label',
        'Count of runs per day. The bands and lines mark single runs in the colour of their outcome.',
    );
    await expect.poll(async () => (await optionOf(page, 'per-day'))?.yAxis?.[0]?.name).toBe('runs');
    await expect(page.locator('[data-legend="per-day"] .item')).toHaveText([
        'each band is one run:',
        'completed',
        'deviated, failed, halted',
        'aborted, interrupted, not ended',
    ]);
    // Each band is drawn in the colour of a swatch, a tenth as strong.
    const bands = ((await optionOf(page, 'per-day'))?.series ?? [])
        .flatMap((series) => series.markArea?.data ?? [])
        .map(([from]) => from?.itemStyle?.color.slice(0, 7));
    expect(bands).toEqual(await swatches(page, 'per-day'));

    await expect(page.getByRole('heading', { level: 2, name: 'Fault injections' })).toBeVisible();
    await expect(page.locator('[data-chart="faults"]')).toHaveAttribute(
        'aria-label',
        'Fault injection steps per kind and plugin',
    );
    await expect
        .poll(async () => (await optionOf(page, 'faults'))?.series?.[0]?.data?.map((d) => d.name))
        .toEqual(['injection / tumult-network', 'injection / tumult-process']);
});

test('overview: the calendar of a week names the two ends of its colour scale', async ({
    page,
}) => {
    const DAY_S = 86_400;
    await mockApi(page, {
        '/api/me': me,
        '/api/overview': {
            ...(bodies['/api/overview'] as object),
            range: '7d',
            from_ns: NOW_NS - 7 * 24 * HOUR_NS,
            experiments_per_day: [
                { ts: NOW_MS / 1000 - 2 * DAY_S, v: 1 },
                { ts: NOW_MS / 1000 - DAY_S, v: 4 },
                { ts: NOW_MS / 1000, v: 2 },
            ],
            faults: [],
        },
        '/api/experiments/windows': { count: 0, runs: [] },
    });
    await page.goto('/?range=7d');

    await expect(page.locator('[data-chart="per-day"]')).toHaveAttribute(
        'aria-label',
        'Count of runs per day as a calendar.',
    );
    await expect(page.locator('[data-legend="per-day"] .item')).toHaveText([
        'runs per day:',
        '0',
        '4',
    ]);
    const map = (await optionOf(page, 'per-day'))?.visualMap?.[0];
    expect({ max: map?.max, ends: [map?.inRange.color[0], map?.inRange.color.at(-1)] }).toEqual({
        max: 4,
        ends: await swatches(page, 'per-day'),
    });
    await expect(
        page.getByText('No fault injection step was recorded in this window.'),
    ).toBeVisible();
});
