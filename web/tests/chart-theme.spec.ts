import { expect, test } from '@playwright/test';
import type { Page } from '@playwright/test';

import { installChartHooks } from './charts';
import { bodies, overview, NOW_MS, NOW_NS, HOUR_NS, DAY_S } from './chart-theme-data';

// The charts of the Overview, Logs, Metrics, Traces, Scores, Resilience
// and Performance pages draw their grid lines, axes, borders, lines and
// tooltips in the colours of the theme in force, and take the colours of
// a new theme when it changes. The fan chart is the score trend of the
// Resilience page.

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

/** The font of the page (--font-body), which chart text is written in. */
const FONT = "-apple-system, 'Segoe UI', Inter, Roboto, sans-serif";

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
        series: ['#5eb1ef', '#E69F00', '#009E73', '#CC79A7', '#56B4E9', '#D55E00', '#F0E442'],
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
        // The Okabe-Ito hues that are below 3:1 on white are darker in light.
        series: ['#1f6fb2', '#c28600', '#009E73', '#CC79A7', '#1d9ae0', '#D55E00', '#9e940d'],
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
    series?: string[];
    font?: string;
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
            color?: string[];
            textStyle?: { fontFamily?: string };
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
            series: o.color,
            font: o.textStyle?.fontFamily,
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
        perf: { ...axes, legend: c.text, mark: c.axis, markLabel: c.text, series: c.series },
    };
}

async function expectFrame(page: Page, id: string, theme: Theme) {
    await expect
        .poll(() => frameOf(page, id), { message: `chart ${id} is drawn in the ${theme} theme` })
        .toMatchObject({ font: FONT, ...framesIn(theme)[id] });
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
