import { expect, test } from '@playwright/test';

import { installChartHooks } from './charts';

// The Metrics chart shows the selected range up to now, and names the
// runs drawn over it without one name running into the next.

const NOW_MS = Date.now();
const NOW_NS = NOW_MS * 1_000_000;
const MINUTE_MS = 60_000;
const HOUR_MS = 60 * MINUTE_MS;
const DAY_MS = 24 * HOUR_MS;

const me = {
    auth_required: false,
    authenticated: true,
    username: null,
    role: null,
    env_scopes: [],
};

const runs = ['first', 'second', 'third'].map((name, i) => ({
    id: `run-${i}`,
    name,
    start_ns: NOW_NS - (30 - 2 * i) * MINUTE_MS * 1_000_000,
    end_ns: NOW_NS - (29 - 2 * i) * MINUTE_MS * 1_000_000,
    outcome: 'completed',
}));

const bodies: Record<string, unknown> = {
    '/api/me': me,
    '/api/metrics/catalog': { metrics: [{ name: 'requests', types: ['sum'], dimensions: [] }] },
    // One bucket: all the 24 hours hold is the last hour.
    '/api/metrics/query': {
        name: 'requests',
        type: 'sum',
        interval: '1h',
        range: '24h',
        group_by: null,
        series: [{ group: null, points: [{ ts: Math.floor((NOW_MS - HOUR_MS) / 1000), v: 7 }] }],
    },
    '/api/experiments/windows': { count: runs.length, runs },
};

interface Chart {
    min: number | undefined;
    max: number | undefined;
    named: string[];
    aligned: (string | undefined)[];
    lines: number;
}

test('metrics: one bucket still shows the whole range and names runs apart', async ({ page }) => {
    await installChartHooks(page);
    await page.route('**/api/**', async (route) => {
        const body = bodies[new URL(route.request().url()).pathname];
        if (body === undefined) await route.fulfill({ status: 404, json: { error: 'not found' } });
        else await route.fulfill({ json: body });
    });
    await page.goto('/metrics?name=requests');
    const chart = () =>
        page.evaluate((): Chart | null => {
            const charts = (window as Window & { __tumultCharts?: Record<string, unknown> })
                .__tumultCharts;
            const option = charts?.['metric'] as
                | {
                      xAxis?: { min?: number; max?: number }[];
                      series?: {
                          markLine?: {
                              data?: { name: string; label?: { show?: boolean; align?: string } }[];
                          };
                      }[];
                  }
                | undefined;
            const data = option?.series?.find((s) => s.markLine)?.markLine?.data;
            if (!data) return null;
            return {
                min: option?.xAxis?.[0]?.min,
                max: option?.xAxis?.[0]?.max,
                named: data.filter((d) => d.label?.show !== false).map((d) => d.name),
                aligned: data.filter((d) => d.label?.show !== false).map((d) => d.label?.align),
                lines: data.length,
            };
        });
    await expect.poll(chart, { message: 'the run overlay is drawn' }).not.toBeNull();
    const seen = await chart();
    // Every run keeps its line; two minutes apart, only the first is named.
    expect(seen?.lines).toBe(3);
    expect(seen?.named).toEqual(['first — completed']);
    // Half an hour before the end of 24 hours: the name ends at its line,
    // so the edge of the chart does not cut it off.
    expect(seen?.aligned).toEqual(['right']);
    // The axis is the selected 24 hours and ends at now, not past it.
    const end = Date.now();
    expect(seen?.max).toBeGreaterThanOrEqual(NOW_MS);
    expect(seen?.max).toBeLessThanOrEqual(end);
    expect((seen?.max ?? 0) - (seen?.min ?? 0)).toBe(DAY_MS);
});
