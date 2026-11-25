import { expect, test } from '@playwright/test';

import { installChartHooks } from './charts';
import { NOW_MS, PAGES, serve } from './style-snapshot-pages';
import type { SnapshotPage } from './style-snapshot-pages';

// The dashed lines of the trace durations chart name p50, p95 and p99 at
// their right end. p95 and p99 are often the same, so the names sit on
// opposite sides of their lines: p99 above, p95 below, never one on the other.

const traces = PAGES.find((target) => target.name === 'traces') as SnapshotPage;

type Mark = { name: string; yAxis: number; label?: { position?: string } };
type Option = { series?: { markLine?: { data?: Mark[] } }[] };

test('the p95 and p99 lines of equal value name themselves on opposite sides', async ({ page }) => {
    await installChartHooks(page);
    await page.clock.setFixedTime(NOW_MS);
    await serve(page, traces);
    await page.goto(traces.path);
    const marks = () =>
        page.evaluate(() => {
            const charts = (window as Window & { __tumultCharts?: Record<string, unknown> })
                .__tumultCharts;
            const option = charts?.['trace-durations'] as Option | undefined;
            return option?.series?.[0]?.markLine?.data ?? [];
        });
    await expect.poll(async () => (await marks()).length).toBe(3);
    const byName = Object.fromEntries((await marks()).map((mark) => [mark.name, mark]));
    expect(byName.p95.yAxis).toBe(byName.p99.yAxis);
    expect(byName.p99.label?.position).toBe('insideEndTop');
    expect(byName.p95.label?.position).toBe('insideEndBottom');
});
