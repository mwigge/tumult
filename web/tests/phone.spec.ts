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
        const target = pageOf('overview');
        await open(page, target);
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
        await page.getByRole('heading', { level: 1, name: target.heading, exact: true }).tap();
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
