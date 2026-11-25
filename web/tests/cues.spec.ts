import { expect, test } from '@playwright/test';

import { show, targetOf } from './keyboard';
import { NOW_MS, PAGES, STATES } from './style-snapshot-pages';

// Colour is never the only signal: an approval that lapses soon, an RTO
// or RPO that missed its target and the status of a span in the waterfall
// each say so in words too. And no page skips a heading level.

test.use({ locale: 'en-US', timezoneId: 'UTC' });

const HOUR_NS = 3_600_000_000_000;
const NOW_NS = NOW_MS * 1_000_000;

// Half an hour from now: within the hour, so soon.
const SOON = NOW_NS + HOUR_NS / 2;

test('an approval that lapses within the hour says soon in the queue', async ({ page }) => {
    const approvals = targetOf('approvals');
    const body = structuredClone(approvals.api['/api/approvals']) as {
        queue: { expires_at_ns: number }[];
    };
    body.queue[0].expires_at_ns = SOON;
    await show(page, { ...approvals, api: { '/api/approvals': body } });
    const ttl = page.getByRole('row').filter({ hasText: 'pg-failover' }).getByRole('cell').nth(6);
    await expect(ttl).toHaveText(/^30m 0s, soon$/);
});

test('an approval that lapses in a day says no more than its time in the queue', async ({
    page,
}) => {
    await show(page, targetOf('approvals'));
    const ttl = page.getByRole('row').filter({ hasText: 'pg-failover' }).getByRole('cell').nth(6);
    await expect(ttl).toHaveText(/^24h 0m$/);
});

test('an approval that lapses within the hour says soon on its run', async ({ page }) => {
    const run = targetOf('run-approval');
    const body = structuredClone(run.api['/api/runs/run-1']) as {
        approval: { request: { expires_at_ns: number } };
    };
    body.approval.request.expires_at_ns = SOON;
    await show(page, { ...run, api: { ...run.api, '/api/runs/run-1': body } });
    const expires = page.locator('.ap-meta div').filter({ hasText: 'Expires' }).locator('b');
    await expect(expires).toHaveText(/^30m 0s, soon$/);
});

test('an RTO or RPO that missed its target says missed, and one that met it does not', async ({
    page,
}) => {
    await show(page, targetOf('dr'));
    const measured = (target: string) =>
        page.locator(`tr[data-target="${target}"]`).getByRole('cell').nth(5);
    await expect(measured('payments-db/prod')).toContainText(
        'at least 30.1 s, not recovered target 10 s, missed',
    );
    await expect(measured('orders/prod')).toHaveText('4 s target 10 s');
});

test('a span in the waterfall says its status in words, not by the colour of its bar alone', async ({
    page,
}) => {
    await show(page, targetOf('trace'));
    const rows = page.locator('.wf button.row');
    const failed = rows.filter({ hasText: 'action' }).first();
    await expect(failed).toHaveAccessibleName(/\berror\b/);
    await expect(failed.locator('.mark')).toBeVisible();
    await expect(failed.locator('.mark')).toHaveText('✕');
    const unset = rows.filter({ hasText: 'experiment' }).first();
    await expect(unset.locator('.mark')).toHaveCount(0);
    await expect(unset).not.toHaveAccessibleName(/\berror\b/);
});

test('the login page has a heading of the first level', async ({ page }) => {
    await show(page, targetOf('login'));
    await expect(
        page.getByRole('heading', { level: 1, name: 'Tumult', exact: true }),
    ).toBeVisible();
});

for (const target of [...PAGES, ...STATES]) {
    const title = target.shows ? `${target.path} with ${target.shows}` : target.path;
    test(`the headings of ${title} go down one level at a time`, async ({ page }) => {
        await show(page, target);
        const levels = await page
            .locator('h1, h2, h3, h4, h5, h6')
            .evaluateAll((els) =>
                els.filter((el) => el.checkVisibility()).map((el) => Number(el.localName[1])),
            );
        const jumps = levels.flatMap((level, i) =>
            level > (i === 0 ? 1 : levels[i - 1] + 1)
                ? [`h${levels[i - 1] ?? 0} to h${level}`]
                : [],
        );
        expect(jumps, `levels ${levels.join(' ')}`).toEqual([]);
    });
}
