import { expect, test } from '@playwright/test';

import { NOW_MS, PAGES, PENDING, Refusal, serve } from './style-snapshot-pages';
import type { SnapshotPage } from './style-snapshot-pages';

// A page whose data does not come tells a screen reader too: a failed
// answer is an alert, and a page that waits for its data shows a busy
// block with a hidden word that says it is loading.

const FAILED = new Refusal(500, { error: 'lake query failed' });

/** The page `name` with `answer` for every call of its API. */
function every(name: string, answer: unknown): SnapshotPage {
    const target = PAGES.find((p) => p.name === name) as SnapshotPage;
    const api = Object.fromEntries(Object.keys(target.api).map((path) => [path, answer]));
    return { ...target, api };
}

const LISTS = ['runs', 'experiments', 'gamedays', 'events'];
const DETAILS = ['run', 'trace', 'experiment', 'gameday'];

for (const name of [...LISTS, ...DETAILS]) {
    const target = every(name, FAILED);
    test(`a failed answer on ${target.path} is an alert with its reason`, async ({ page }) => {
        await page.clock.setFixedTime(NOW_MS);
        await serve(page, target);
        await page.goto(target.path);

        const alert = page.getByRole('alert').filter({ hasText: 'lake query failed' });
        await expect(alert.first()).toBeVisible();
    });
}

for (const name of ['runs', 'trace']) {
    const target = every(name, PENDING);
    test(`${target.path} is busy and says it is loading while its answer is on its way`, async ({
        page,
    }) => {
        await page.clock.setFixedTime(NOW_MS);
        await serve(page, target);
        await page.goto(target.path);

        const busy = page.locator('[aria-busy="true"]');
        await expect(busy.first()).toBeVisible();
        await expect(busy.first()).toHaveText('Loading');
    });
}
