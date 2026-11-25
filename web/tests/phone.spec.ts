import { expect, test } from '@playwright/test';
import type { Page } from '@playwright/test';

import { holds, NOW_MS, PAGES, serve, STATES } from './style-snapshot-pages';
import type { SnapshotPage } from './style-snapshot-pages';

// Krönika works on a phone, 390 x 844: no page scrolls sideways, the
// side bar folds behind a menu button in a top bar, and a wide table
// scrolls inside its panel.

const PHONE = { width: 390, height: 844 };

test.use({ viewport: PHONE, locale: 'en-US', timezoneId: 'UTC' });

const pageOf = (name: string) => PAGES.find((target) => target.name === name) as SnapshotPage;

/** Open `target` as the style snapshot does: with its answers, its heading and its clicks. */
async function open(page: Page, target: SnapshotPage) {
    await page.clock.setFixedTime(NOW_MS);
    await serve(page, target);
    await page.goto(target.path);
    if (target.heading)
        await expect(
            page.getByRole('heading', { level: 1, name: target.heading, exact: true }),
        ).toBeVisible();
    // A page that waits for an answer never goes idle.
    if (!holds(target)) await page.waitForLoadState('networkidle');
    if (target.open) {
        await target.open(page);
        await page.waitForLoadState('networkidle');
    }
}

/** How far the page reaches past the right edge of the window, once it holds still. */
async function overflow(page: Page): Promise<number> {
    const read = () =>
        page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
    let last = await read();
    await expect
        .poll(
            async () => {
                const now = await read();
                const same = now === last;
                last = now;
                return same;
            },
            { message: 'the width of the page holds still', intervals: [250], timeout: 10_000 },
        )
        .toBe(true);
    return last;
}

/** The elements that reach past the right edge of the window further than their parent does. */
const outside = (page: Page) =>
    page.evaluate(() => {
        const edge = window.innerWidth;
        return [...document.querySelectorAll('body *')]
            .filter((el) => {
                const right = el.getBoundingClientRect().right;
                const parent = el.parentElement?.getBoundingClientRect().right ?? 0;
                return right > edge && right > parent && el.checkVisibility();
            })
            .map((el) => `${el.localName}.${[...el.classList].join('.')}`)
            .slice(0, 10);
    });

/** The colour of each pixel of row `y` of a picture, as `rgb(r, g, b)`. The browser reads the PNG. */
const pixelRow = (page: Page, png: Buffer, y: number) =>
    page.evaluate(
        async ({ bytes, y }) => {
            const blob = new Blob([new Uint8Array(bytes)], { type: 'image/png' });
            const bitmap = await createImageBitmap(blob);
            const canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
            const context = canvas.getContext('2d') as OffscreenCanvasRenderingContext2D;
            context.drawImage(bitmap, 0, 0);
            const { data } = context.getImageData(0, y, bitmap.width, 1);
            return Array.from(
                { length: bitmap.width },
                (_, x) => `rgb(${data[4 * x]}, ${data[4 * x + 1]}, ${data[4 * x + 2]})`,
            );
        },
        { bytes: [...png], y },
    );

for (const target of [...PAGES, ...STATES]) {
    const title = target.shows ? `${target.path} with ${target.shows}` : target.path;
    test(`${title} does not scroll sideways on a phone`, async ({ page }) => {
        await open(page, target);

        const past = await overflow(page);
        expect(past, `past the edge: ${(await outside(page)).join(', ')}`).toBeLessThanOrEqual(0);
    });
}

test('the menu button opens the side bar and closes it again', async ({ page }) => {
    await open(page, pageOf('overview'));
    const menu = page.getByRole('button', { name: 'Menu' });
    const nav = page.getByRole('navigation');

    await expect(menu).toHaveAttribute('aria-expanded', 'false');
    await expect(nav).toBeHidden();
    const controls = await menu.getAttribute('aria-controls');
    expect(controls, 'the button names what it opens').toBeTruthy();

    await menu.click();
    await expect(menu).toHaveAttribute('aria-expanded', 'true');
    await expect(nav).toBeVisible();
    await expect(page.locator(`[id="${controls}"]`)).toBeVisible();
    await expect(page.locator(`[id="${controls}"]`).getByRole('navigation')).toBeVisible();

    await menu.click();
    await expect(menu).toHaveAttribute('aria-expanded', 'false');
    await expect(nav).toBeHidden();
});

test('Escape closes the menu and gives the focus back to its button', async ({ page }) => {
    await open(page, pageOf('overview'));
    const menu = page.getByRole('button', { name: 'Menu' });
    const nav = page.getByRole('navigation');

    await menu.click();
    await nav.getByRole('link', { name: /Runs/ }).focus();
    await page.keyboard.press('Escape');

    await expect(nav).toBeHidden();
    await expect(menu).toHaveAttribute('aria-expanded', 'false');
    await expect(menu).toBeFocused();
});

test('choosing an entry of the menu opens its page and closes the menu', async ({ page }) => {
    const overview = pageOf('overview');
    await open(page, { ...overview, api: { ...overview.api, ...pageOf('runs').api } });
    const menu = page.getByRole('button', { name: 'Menu' });
    const nav = page.getByRole('navigation');

    await menu.click();
    await nav.getByRole('link', { name: /Runs/ }).click();

    await expect(page).toHaveURL(/\/runs$/);
    await expect(page.getByRole('heading', { level: 1, name: 'Runs', exact: true })).toBeVisible();
    await expect(nav).toBeHidden();
    await expect(menu).toHaveAttribute('aria-expanded', 'false');
});

test.describe('on a touch screen', () => {
    test.use({ hasTouch: true });

    test('a tap outside the open menu closes it, and a tap in it does not', async ({ page }) => {
        await open(page, pageOf('overview'));
        const menu = page.getByRole('button', { name: 'Menu' });
        const nav = page.getByRole('navigation');
        const side = page.locator('#side');
        const focusIn = (selector: string) =>
            page.evaluate(
                (s) => document.querySelector(s)?.contains(document.activeElement) ?? false,
                selector,
            );

        await menu.tap();
        await expect(nav).toBeVisible();
        await side.tap({ position: { x: 4, y: 4 } });
        await expect(nav).toBeVisible();
        await expect(menu).toHaveAttribute('aria-expanded', 'true');

        await nav.getByRole('link', { name: /Runs/ }).focus();
        await page.getByRole('heading', { level: 1, name: 'Overview', exact: true }).tap();
        await expect(nav).toBeHidden();
        await expect(menu).toHaveAttribute('aria-expanded', 'false');
        // The focus goes where the tap went: not into the hidden menu, and
        // not back up to the menu button.
        expect(await focusIn('#side')).toBe(false);
        await expect(menu).not.toBeFocused();

        // The menu button is not outside the menu: it still opens and closes it.
        await menu.tap();
        await expect(nav).toBeVisible();
        await menu.tap();
        await expect(nav).toBeHidden();
    });
});

test('on a wide screen the side bar shows and the menu button does not', async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await open(page, pageOf('overview'));

    await expect(page.getByRole('navigation')).toBeVisible();
    await expect(page.getByRole('button', { name: 'Menu' })).toBeHidden();
});

for (const name of ['runs', 'schedules']) {
    const target = pageOf(name);
    test(`the wide table of ${target.path} scrolls inside its panel`, async ({ page }) => {
        await open(page, target);
        const table = page.locator('table.data').first();

        // A box between the table and the edge of its panel scrolls sideways.
        const scrolls = await table.evaluate((el) => {
            const panel = el.closest('.panel');
            for (let box = el.parentElement; box && panel?.contains(box); box = box.parentElement) {
                const { overflowX } = getComputedStyle(box);
                const scroller = overflowX === 'auto' || overflowX === 'scroll';
                if (scroller && box.scrollWidth > box.clientWidth) return true;
            }
            return false;
        });
        expect(scrolls, 'the table is wider than its panel, and the panel scrolls').toBe(true);
        expect(await overflow(page)).toBeLessThanOrEqual(0);
    });
}

for (const name of ['runs', 'events', 'manual']) {
    const target = pageOf(name);
    test(`each row of the wrapped group of ${target.path} starts with one line`, async ({
        page,
    }) => {
        await open(page, target);
        const group = page.locator('.seg').first();
        // Each row of buttons: the middle of its height, and where its first
        // and second buttons start, from the left edge of the group.
        const { line, rows } = await group.evaluate((el) => {
            const box = el.getBoundingClientRect();
            const byRow = new Map<number, number[]>();
            for (const button of el.querySelectorAll('button')) {
                const at = button.getBoundingClientRect();
                const top = Math.round(at.top - box.top);
                byRow.set(top, [...(byRow.get(top) ?? []), at.left - box.left]);
            }
            const height = (el.querySelector('button') as Element).getBoundingClientRect().height;
            return {
                line: getComputedStyle(el).borderLeftColor,
                rows: [...byRow].map(([top, lefts]) => ({
                    y: Math.round(top + height / 2),
                    second: lefts.length > 1 ? Math.round(lefts[1]) : null,
                })),
            };
        });
        expect(rows.length, 'the group wraps at phone width').toBeGreaterThan(1);

        const png = await group.screenshot();
        for (const [index, row] of rows.entries()) {
            const pixels = await pixelRow(page, png, row.y);
            const lines = (from: number, to: number) =>
                pixels.slice(Math.max(0, from), to).filter((pixel) => pixel === line).length;
            expect(lines(0, 4), `the left edge of row ${index + 1} is one line`).toBe(1);
            if (row.second === null) continue;
            expect(
                lines(row.second - 3, row.second + 3),
                `the line between the first two buttons of row ${index + 1} is one line`,
            ).toBe(1);
        }
    });
}
