import { expect, test } from '@playwright/test';
import type { Locator } from '@playwright/test';

import { show, tabTo, targetOf } from './keyboard';

// The first Tab stop of a page is a link past the navigation to the main
// content. The navigation has a name and marks the page it shows. A field
// shows a ring in the accent colour when the keyboard gives it the focus.

test.use({ locale: 'en-US', timezoneId: 'UTC' });

test('the first Tab stop is a link that moves the focus past the navigation to the main content', async ({
    page,
}) => {
    await show(page, targetOf('runs'));
    const skip = page.getByRole('link', { name: 'Skip to main content' });
    await expect(skip).toBeAttached();
    // Out of sight until it has the focus.
    expect((await skip.boundingBox())?.width ?? 0).toBeLessThanOrEqual(1);
    await page.keyboard.press('Tab');
    await expect(skip).toBeFocused();
    expect((await skip.boundingBox())?.width ?? 0).toBeGreaterThan(1);
    await expect(skip).toBeInViewport();
    await page.keyboard.press('Enter');
    await expect(page.getByRole('main')).toBeFocused();
    // The next stop is in the content, not in the navigation.
    await page.keyboard.press('Tab');
    expect(await page.evaluate(() => document.activeElement?.closest('main') !== null)).toBe(true);
});

for (const { name, current, other } of [
    { name: 'runs', current: 'Runs', other: 'Overview' },
    { name: 'scores', current: 'Scores', other: 'DORA' },
    { name: 'dora', current: 'DORA', other: 'Scores' },
]) {
    test(`the named navigation marks ${current} as the current page on ${targetOf(name).path}`, async ({
        page,
    }) => {
        await show(page, targetOf(name));
        const nav = page.getByRole('navigation', { name: 'Main' });
        await expect(nav.getByRole('link', { name: current })).toHaveAttribute(
            'aria-current',
            'page',
        );
        await expect(nav.getByRole('link', { name: other })).not.toHaveAttribute(
            'aria-current',
            /.*/,
        );
        await expect(nav.locator('[aria-current]')).toHaveCount(1);
    });
}

/** The outline of `field`, and the colour of the accent of the theme. */
const ringOf = (field: Locator) =>
    field.evaluate((el) => {
        const style = getComputedStyle(el);
        const probe = document.createElement('span');
        probe.style.color = 'var(--accent)';
        document.body.append(probe);
        const accent = getComputedStyle(probe).color;
        probe.remove();
        return {
            style: style.outlineStyle,
            width: style.outlineWidth,
            color: style.outlineColor,
            accent,
        };
    });

test('a field shows a ring in the accent colour when the keyboard gives it the focus', async ({
    page,
}) => {
    await show(page, targetOf('experiments'));
    const fields = [page.getByRole('searchbox').first(), page.getByRole('combobox').first()];
    for (const field of fields) {
        await tabTo(page, field);
        const ring = await ringOf(field);
        expect(ring.style).toBe('solid');
        expect(ring.width).toBe('2px');
        expect(ring.color).toBe(ring.accent);
    }
});

test('a text area shows the same ring when the keyboard gives it the focus', async ({ page }) => {
    await show(page, targetOf('gamedays'));
    const field = page.getByRole('textbox', { name: /Campaign TOON/ });
    await tabTo(page, field);
    const ring = await ringOf(field);
    expect([ring.style, ring.width, ring.color]).toEqual(['solid', '2px', ring.accent]);
});

test('the heading the Star Map drawer gives the focus shows the same ring', async ({ page }) => {
    await show(page, targetOf('starmap'));
    const list = page.getByRole('region', { name: 'nodes' });
    await tabTo(page, list.getByRole('button').first());
    await page.keyboard.press('Enter');
    const heading = page.getByRole('dialog').getByRole('heading').first();
    await expect(heading).toBeFocused();
    const ring = await ringOf(heading);
    expect([ring.style, ring.width, ring.color]).toEqual(['solid', '2px', ring.accent]);
});
