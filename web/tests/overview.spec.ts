import { expect, test } from '@playwright/test';

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
