import { expect, test } from '@playwright/test';
import type { Page } from '@playwright/test';

import { PAGES, serve } from './style-snapshot-pages';

// A saved theme is on the page from its first paint. The page has no
// server render, so only the head of app.html runs before the app mounts:
// a small static script there puts the saved theme on <html>, before the
// stylesheet loads. A user who chose light never sees the dark default
// first, and each chart is drawn once, in the theme in force.

/** The background of the page in each theme: --bg of lib/tokens.css. */
const BG = { dark: 'rgb(11, 14, 17)', light: 'rgb(244, 246, 248)' };

/** The tooltip of a chart in each theme: its panel colour. */
const TOOLTIP = { dark: '#151b21', light: '#ffffff' };

/** What the page was at DOMContentLoaded. */
interface FirstPaint {
    theme: string | null;
    bg: string;
    mounted: boolean;
}

interface Recorder {
    __firstPaint?: FirstPaint;
    __themes?: (string | null)[];
}

/**
 * Open `name` of the mocked pages, with `saved` as the theme the user
 * chose, or with none. The page records what it was at DOMContentLoaded,
 * before the app mounts, and every value `data-theme` takes on <html>.
 */
async function open(page: Page, name: string, saved: string | null) {
    await page.addInitScript((theme) => {
        if (theme) localStorage.setItem('tumult.theme', theme);
        const w = window as Window & Recorder;
        const themes: (string | null)[] = [];
        w.__themes = themes;
        new MutationObserver((records) => {
            for (const r of records) themes.push((r.target as Element).getAttribute('data-theme'));
        }).observe(document, { subtree: true, attributes: true, attributeFilter: ['data-theme'] });
        document.addEventListener('DOMContentLoaded', () => {
            w.__firstPaint = {
                theme: document.documentElement.getAttribute('data-theme'),
                bg: getComputedStyle(document.body).backgroundColor,
                mounted: document.querySelector('button.theme') !== null,
            };
        });
    }, saved);
    const target = PAGES.find((p) => p.name === name);
    if (!target) throw new Error(`no mocked page ${name}`);
    const missing = await serve(page, target);
    await page.goto(target.path);
    return missing;
}

const firstPaint = (page: Page) =>
    page.evaluate(() => (window as Window & Recorder).__firstPaint ?? null);

/** Each value `data-theme` took, once, in the order it came. */
const themesTaken = (page: Page) =>
    page.evaluate(() => [...new Set((window as Window & Recorder).__themes)]);

test('a saved light theme is on the page at its first paint, before the app mounts', async ({
    page,
}) => {
    await open(page, 'overview', 'light');
    const button = page.locator('button.theme');
    await expect(button, 'the app mounts').toBeVisible();

    expect(await firstPaint(page)).toEqual({ theme: 'light', bg: BG.light, mounted: false });
    expect(await themesTaken(page), 'the page is never dark').toEqual(['light']);

    // The theme button starts from the saved theme and goes on from it, as before.
    await expect(button).toContainText('light');
    await button.click();
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'auto');
    await expect(button).toContainText('auto');
});

test.describe('on a system in light mode', () => {
    test.use({ colorScheme: 'light' });

    test('the auto theme has no dark first paint', async ({ page }) => {
        await open(page, 'overview', 'auto');
        await expect(page.locator('button.theme'), 'the app mounts').toBeVisible();

        expect(await firstPaint(page)).toEqual({ theme: 'auto', bg: BG.light, mounted: false });
        expect(await themesTaken(page)).toEqual(['auto']);
        await expect(page.locator('button.theme')).toContainText('auto');
    });

    test('with no saved theme the page is dark, the default', async ({ page }) => {
        await open(page, 'overview', null);
        await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');

        expect(await firstPaint(page)).toEqual({ theme: null, bg: BG.dark, mounted: false });
        expect(await themesTaken(page)).toEqual(['dark']);
        const bg = await page.evaluate(() => getComputedStyle(document.body).backgroundColor);
        expect(bg).toBe(BG.dark);
        await expect(page.locator('button.theme')).toContainText('dark');
    });
});

interface ChartLog {
    __tumultCharts?: Record<string, unknown>;
    /** How many times each chart was made, by its id: one `echarts.init` each. */
    __inits?: Record<string, number>;
    /** The tooltip colour of the first option each chart showed, by its id. */
    __firstTooltip?: Record<string, string | null>;
}

/**
 * Count the charts each chart host makes, and keep the first option each
 * shows. ECharts marks the element of a chart it makes with a new
 * `_echarts_instance_`, and clears it when the chart is disposed.
 */
async function logCharts(page: Page) {
    await page.addInitScript(() => {
        const w = window as Window & ChartLog;
        const inits: Record<string, number> = {};
        const first: Record<string, string | null> = {};
        w.__inits = inits;
        w.__firstTooltip = first;
        new MutationObserver((records) => {
            for (const r of records) {
                const id = (r.target as Element).getAttribute('data-chart');
                if (id && !r.oldValue) inits[id] = (inits[id] ?? 0) + 1;
            }
        }).observe(document, {
            subtree: true,
            attributes: true,
            attributeOldValue: true,
            attributeFilter: ['_echarts_instance_'],
        });
        type Option = { tooltip?: { backgroundColor?: string }[] };
        w.__tumultCharts = new Proxy({} as Record<string, unknown>, {
            set(charts, key, value) {
                if (typeof key === 'string' && !key.includes('#') && !(key in first))
                    first[key] = (value as Option)?.tooltip?.[0]?.backgroundColor ?? null;
                charts[key as string] = value;
                return true;
            },
        });
    });
}

const DRAWN: { name: string; charts: string[] }[] = [
    { name: 'overview', charts: ['per-day', 'faults'] },
    { name: 'resilience', charts: ['coverage', 'coverage-radar', 'trend'] },
];

for (const { name, charts } of DRAWN) {
    test(`each chart of the ${name} page is drawn once, in the light theme, on a light load`, async ({
        page,
    }) => {
        await logCharts(page);
        const missing = await open(page, name, 'light');
        const shown = () =>
            page.evaluate(() => Object.keys((window as Window & ChartLog).__firstTooltip ?? {}));
        await expect.poll(shown).toEqual(expect.arrayContaining(charts));
        await page.waitForLoadState('networkidle');
        expect(missing, 'every API call of the page has an answer').toEqual([]);

        const log = await page.evaluate(() => {
            const w = window as Window & ChartLog;
            return { inits: w.__inits, first: w.__firstTooltip };
        });
        for (const id of charts) {
            expect(log.inits?.[id], `chart ${id} is made once`).toBe(1);
            expect(log.first?.[id], `chart ${id} is light from its first option`).toBe(
                TOOLTIP.light,
            );
        }
    });
}
