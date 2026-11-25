import { expect, test } from '@playwright/test';
import type { Locator } from '@playwright/test';

import { NOW_MS, PAGES, serve } from './style-snapshot-pages';
import type { SnapshotPage } from './style-snapshot-pages';

// The foot of the 208 px side bar names who signed in: the name, a badge of
// the role and the Log out button, side by side on one row.

test.use({ viewport: { width: 1440, height: 900 } });

const runs = PAGES.find((target) => target.name === 'runs') as SnapshotPage;

/** The Runs page, signed in as `username` with `role`. */
const signedIn = (username: string, role: string): SnapshotPage => ({
    ...runs,
    api: {
        ...runs.api,
        '/api/me': { auth_required: true, authenticated: true, username, role, env_scopes: [] },
    },
});

/** The number of lines the text of `part` takes. */
const lines = (part: Locator) =>
    part.evaluate((el) => {
        const range = document.createRange();
        range.selectNodeContents(el);
        return new Set([...range.getClientRects()].map((rect) => Math.round(rect.top))).size;
    });

test('the user chip shows the whole name, the role and Log out on one line', async ({ page }) => {
    await page.clock.setFixedTime(NOW_MS);
    await serve(page, signedIn('admin', 'admin'));
    await page.goto(runs.path);
    const side = page.locator('aside.side');
    const chip = side.locator('.user-chip');
    await expect(chip).toBeVisible();
    expect((await side.boundingBox())?.width).toBe(208);

    const name = chip.locator('.username');
    await expect(name).toHaveText('admin');
    expect(await name.evaluate((el) => el.scrollWidth <= el.clientWidth)).toBe(true);

    const logout = chip.getByRole('button', { name: 'Log out' });
    expect(await lines(logout)).toBe(1);

    const badge = chip.locator('.badge');
    await expect(badge).toHaveText('admin');
    const [badgeBox, logoutBox, chipBox] = await Promise.all(
        [badge, logout, chip].map((part) => part.boundingBox()),
    );
    expect(badgeBox!.x + badgeBox!.width).toBeLessThanOrEqual(logoutBox!.x);
    expect(logoutBox!.x + logoutBox!.width).toBeLessThanOrEqual(chipBox!.x + chipBox!.width);
});
