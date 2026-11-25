import { expect, test } from '@playwright/test';

import { NOW_MS, PAGES, serve } from './style-snapshot-pages';

// The pages that no other spec opens load with their mocked API, show
// their heading and write no error to the console.

const UNTESTED = [
    'approvals',
    'ask',
    'author',
    'author-new',
    'events',
    'login',
    'schedules',
    'users',
    'webhooks',
];

for (const target of PAGES.filter((p) => UNTESTED.includes(p.name))) {
    test(`${target.path} loads and shows its heading with no error`, async ({ page }) => {
        const errors: string[] = [];
        page.on('console', (message) => {
            if (message.type() === 'error') errors.push(message.text());
        });
        page.on('pageerror', (error) => errors.push(error.message));
        await page.clock.setFixedTime(NOW_MS);
        const missing = await serve(page, target);
        await page.goto(target.path);

        await expect(
            page.getByRole('heading', { level: 1, name: target.heading ?? '', exact: true }),
        ).toBeVisible();
        await page.waitForLoadState('networkidle');
        expect(missing, 'every API call of the page has an answer').toEqual([]);
        expect(errors, 'the console has no error').toEqual([]);
    });
}

test('every page on the smoke list is a page with mocks', () => {
    const names = PAGES.map((p) => p.name);
    expect(UNTESTED.filter((name) => !names.includes(name))).toEqual([]);
});
