import { expect } from '@playwright/test';
import type { Locator, Page } from '@playwright/test';

// A chart is a canvas, but its tooltip is HTML next to it. These helpers
// show the tooltip of one item the way a pointer over it would, and
// assert that a name with markup in it is shown as text.

/** A name that would load an image and run script if it were read as markup. */
export const EVIL = '<img src=x onerror="window.__xss = 1">';

/**
 * Which item of a chart: what `showTip` of ECharts takes, or with `cell`
 * the place of a value on the axes of the series (a heatmap cell).
 */
export interface TipTarget {
    seriesIndex: number;
    dataIndex?: number;
    dataType?: 'node' | 'edge';
    cell?: [number, number];
}

// The one registry: the option of a chart under its id, and under
// `<id>#tip` the function that shows one of its tooltips.
interface Hooks {
    __tumultCharts?: Record<string, unknown>;
    __xss?: number;
}

/**
 * Install what makes charts mirror their option and offer their
 * tooltips. A chart does neither where no test did this.
 */
export async function installChartHooks(page: Page) {
    await page.addInitScript(() => {
        (window as Window & Hooks).__tumultCharts = {};
    });
}

/** The tooltip element of a chart: ECharts puts it after the canvas layer. */
export const tooltipOf = (page: Page, id: string): Locator =>
    page.locator(`[data-chart="${id}"] > div`).nth(1);

/** Show the tooltip of one item of the chart `id`. */
export async function showTip(page: Page, id: string, at: TipTarget): Promise<Locator> {
    const drawn = (chart: string) => {
        const w = window as Window & Hooks;
        const option = w.__tumultCharts?.[chart] as { series?: unknown[] } | undefined;
        return (option?.series?.length ?? 0) > 0 && Boolean(w.__tumultCharts?.[`${chart}#tip`]);
    };
    await expect
        .poll(() => page.evaluate(drawn, id), { message: `chart ${id} is drawn` })
        .toBe(true);
    await page.evaluate(
        ([chart, target]) => {
            const show = (window as Window & Hooks).__tumultCharts?.[`${chart}#tip`];
            (show as (at: typeof target) => void)(target);
        },
        [id, at] as const,
    );
    const tip = tooltipOf(page, id);
    await expect(tip).toBeVisible();
    return tip;
}

/** `text` is on the page as text: no element came of it and no script ran. */
export async function expectShownAsText(page: Page, tip: Locator, text: string = EVIL) {
    await expect(tip).toContainText(text);
    await expect(tip.locator('img')).toHaveCount(0);
    await expect(page.locator('img[src="x"]')).toHaveCount(0);
    expect(await page.evaluate(() => (window as Window & Hooks).__xss)).toBe(undefined);
}
