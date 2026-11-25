// What the keyboard, focus and cue specs share: a mocked page shown with
// its data, and Tab pressed until an element has the focus.
import { expect } from '@playwright/test';
import type { Locator, Page } from '@playwright/test';

import { holds, NOW_MS, PAGES, serve, STATES } from './style-snapshot-pages';
import type { SnapshotPage } from './style-snapshot-pages';

/** The mocked page or state `name`, with its own path and answers where given. */
export function targetOf(name: string, change: Partial<SnapshotPage> = {}): SnapshotPage {
    const target = [...PAGES, ...STATES].find((p) => p.name === name);
    if (!target) throw new Error(`no mocked page ${name}`);
    return { ...target, ...change, api: { ...target.api, ...change.api } };
}

/** Show `target` with its data, and the state its clicks open. */
export async function show(page: Page, target: SnapshotPage) {
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

/** Press Tab until `target` has the focus, and fail when it never does. */
export async function tabTo(page: Page, target: Locator) {
    await expect(target).toBeVisible();
    for (let presses = 0; presses < 150; presses++) {
        await page.keyboard.press('Tab');
        if (await target.evaluate((el) => el === document.activeElement)) return;
    }
    throw new Error('Tab never reached the element');
}
