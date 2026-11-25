// The pictures of docs/reference/ui-standard.md: each part of Krönika on a
// mocked page of the style snapshot, light theme, cut to the part. Run with
// `npm run shots:ui-standard` in web/.
import { expect, test } from '@playwright/test';
import type { Locator, Page } from '@playwright/test';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

import { NOW_MS, PAGES, PENDING, STATES, serve } from '../tests/style-snapshot-pages';
import type { SnapshotPage } from '../tests/style-snapshot-pages';

const OUT =
    process.env.SHOTS_OUT ??
    fileURLToPath(new URL('../../docs/images/3.0/ui-standard/', import.meta.url));
const ALL = [...PAGES, ...STATES];
const PAD = 12;

test.use({ viewport: { width: 1200, height: 800 }, deviceScaleFactor: 2 });

function named(name: string): SnapshotPage {
    const found = ALL.find((p) => p.name === name);
    if (!found) throw new Error(`no mocked page ${name}`);
    return found;
}

async function open(page: Page, target: SnapshotPage) {
    await page.addInitScript(() => localStorage.setItem('tumult.theme', 'light'));
    await page.clock.setFixedTime(NOW_MS);
    const missing = await serve(page, target);
    await page.goto(target.path);
    if (target.heading) await expect(page.locator('h1').first()).toContainText(target.heading);
    if (target.open) await target.open(page);
    // The charts draw after their data comes.
    await page.waitForTimeout(800);
    expect(missing).toEqual([]);
}

/** A picture of the box around `parts`, with a margin. */
async function crop(page: Page, file: string, parts: Locator[], pad = PAD) {
    const boxes = [];
    for (const part of parts) {
        await expect(part).toBeVisible();
        const box = await part.boundingBox();
        if (!box) throw new Error(`${file}: a part has no box`);
        boxes.push(box);
    }
    const x = Math.max(0, Math.min(...boxes.map((b) => b.x)) - pad);
    const y = Math.max(0, Math.min(...boxes.map((b) => b.y)) - pad);
    const right = Math.max(...boxes.map((b) => b.x + b.width)) + pad;
    const bottom = Math.max(...boxes.map((b) => b.y + b.height)) + pad;
    await page.screenshot({
        path: join(OUT, file),
        clip: { x, y, width: right - x, height: bottom - y },
        animations: 'disabled',
    });
}

test('badge', async ({ page }) => {
    await open(page, named('runs'));
    const badges = page.locator('table.data tbody tr td:first-child .badge');
    await expect(badges).toHaveCount(3);
    await crop(page, 'badge.png', [badges.nth(0), badges.nth(2)]);
});

test('page header', async ({ page }) => {
    await page.setViewportSize({ width: 1000, height: 800 });
    await open(page, named('run'));
    const parts = page.locator('.page-head').first().locator(':scope > *');
    await crop(page, 'page-header.png', await parts.all());
});

test('panel', async ({ page }) => {
    await open(page, named('overview'));
    await crop(page, 'panel.png', [page.locator('.panel', { hasText: 'Runs per day' }).first()]);
});

test('segmented', async ({ page }) => {
    await open(page, named('overview'));
    await crop(page, 'segmented.png', [page.getByRole('group', { name: 'time range' })]);
});

test('tabs', async ({ page }) => {
    await open(page, named('manual'));
    await crop(page, 'tabs.png', [page.getByRole('tablist')]);
});

test('meta row', async ({ page }) => {
    await page.setViewportSize({ width: 1000, height: 800 });
    await open(page, named('trace'));
    await crop(page, 'meta-row.png', [
        page.locator('main .panel.meta').first(),
        page.locator('main .panel.meta').nth(3),
    ]);
});

test('drawer', async ({ page }) => {
    await page.setViewportSize({ width: 1200, height: 420 });
    await open(page, named('trace-drawer'));
    const box = await page.locator('dialog.drawer').boundingBox();
    if (!box) throw new Error('drawer.png: the drawer has no box');
    await page.screenshot({
        path: join(OUT, 'drawer.png'),
        clip: { x: box.x - 160, y: 0, width: box.width + 160, height: 420 },
        animations: 'disabled',
    });
});

test('state empty', async ({ page }) => {
    await page.setViewportSize({ width: 1000, height: 800 });
    await open(page, { ...named('runs'), api: { '/api/runs': { count: 0, runs: [] } } });
    await crop(page, 'state-empty.png', [
        page.locator('main .panel', { has: page.locator('.state') }).first(),
    ]);
});

test('state error', async ({ page }) => {
    await page.setViewportSize({ width: 1000, height: 800 });
    await open(page, named('runs-error'));
    await crop(page, 'state-error.png', [
        page.locator('main .panel', { has: page.getByRole('alert') }).first(),
    ]);
});

test.describe('at scale 1', () => {
    test.use({ viewport: { width: 1000, height: 800 }, deviceScaleFactor: 1 });
    test('state loading', async ({ page }) => {
        await open(page, { ...named('runs'), heading: null, api: { '/api/runs': PENDING } });
        await crop(page, 'state-loading.png', [
            page.locator('main .panel', { has: page.locator('.skeleton') }).first(),
        ]);
    });
});

test('skip link', async ({ page }) => {
    await open(page, named('runs'));
    await page.keyboard.press('Tab');
    const skip = page.locator('.skip-link');
    await expect(skip).toBeFocused();
    await crop(page, 'skip-link.png', [skip, page.locator('h1').first()], 16);
});

test('focus ring', async ({ page }) => {
    await open(page, named('manual'));
    const field = page.getByLabel('Experiment name');
    await field.focus();
    await crop(page, 'focus-ring.png', [page.locator('label', { has: field }).first()]);
});

test.describe('phone', () => {
    test.use({ viewport: { width: 390, height: 640 }, deviceScaleFactor: 2 });
    test('phone menu', async ({ page }) => {
        await open(page, named('runs'));
        const closed = await page.screenshot({ animations: 'disabled' });
        const menu = page.getByRole('button', { name: 'Menu' });
        await menu.click();
        await expect(menu).toHaveAttribute('aria-expanded', 'true');
        const opened = await page.screenshot({ animations: 'disabled' });
        // The two screens side by side, on the white of the docs page.
        const img = (png: Buffer) =>
            `<img width="390" height="640" style="border:1px solid #c9d1d9" src="data:image/png;base64,${png.toString('base64')}">`;
        await page.setViewportSize({ width: 838, height: 674 });
        await page.goto('about:blank');
        await page.setContent(
            `<body style="margin:0;padding:16px;background:#ffffff;display:flex;gap:24px">${img(closed)}${img(opened)}</body>`,
        );
        await page.screenshot({ path: join(OUT, 'phone-menu.png') });
    });
});
