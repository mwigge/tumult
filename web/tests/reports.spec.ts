import { expect, test } from '@playwright/test';
import type { Page } from '@playwright/test';

import { DAY_MS, drTargets } from './dr-data';
import { records } from './manual-data';

// The Reports page makes each compliance report of the API, R1 to R5, and
// posts the body the API takes for it. The API is mocked.

const me = {
    auth_required: false,
    authenticated: true,
    username: null,
    role: null,
    env_scopes: [],
};

const NOW = Date.UTC(2026, 10, 25, 12);

const meta = {
    doc_id: 'doc-1',
    type: 'dr-test',
    title: 'DR test report',
    created_ns: NOW * 1e6,
    data_as_of_ns: NOW * 1e6,
    bytes: 2048,
    sha256: 'ab'.repeat(32),
    params: { period: null, experiment_id: null, framework: null },
};

/** Mocks `/api` and returns the bodies posted to make a report. */
async function mockApi(page: Page): Promise<unknown[]> {
    const posted: unknown[] = [];
    await page.clock.setFixedTime(NOW);
    await page.route('**/api/**', async (route) => {
        const path = new URL(route.request().url()).pathname;
        let body: unknown = {};
        if (path === '/api/me') body = me;
        else if (path === '/api/reports/v2/generate') {
            posted.push(route.request().postDataJSON());
            body = meta;
        } else if (path === '/api/reports/v2' || path === '/api/reports') body = { reports: [] };
        else if (path === '/api/experiments') body = { experiments: [] };
        else if (path === '/api/metrics') body = { metrics: [] };
        else if (path === '/api/dr') body = { targets: drTargets(NOW), tests: [] };
        else if (path === '/api/manual/experiments') body = { records };
        await route.fulfill({ json: body });
    });
    return posted;
}

/** Choose the template of `label`, which the page must offer. */
async function pickTemplate(page: Page, label: string) {
    const template = page.getByLabel('template');
    await expect(template.locator('option'), 'the templates of the page').toContainText([label]);
    await template.selectOption({ label });
}

const generate = (page: Page) =>
    page.getByRole('button', { name: 'Generate', exact: true }).first().click();

test('the R4 form posts a target of the DR register, its environment and the period', async ({
    page,
}) => {
    const posted = await mockApi(page);
    await page.goto('/reports');
    await pickTemplate(page, 'R4 · DR test report');
    const target = page.getByLabel('target', { exact: true });
    await expect(target.locator('option')).toHaveText([
        'archive · prod',
        'billing · prod',
        'catalog · prod',
        'ledger · prod',
        'orders · prod',
        'payments-db · prod',
        'search · staging',
    ]);
    await target.selectOption({ label: 'search · staging' });
    await page.getByLabel('period').selectOption({ label: '90 days' });
    await generate(page);

    await expect.poll(() => posted).toHaveLength(1);
    expect(posted[0]).toEqual({
        type: 'dr-test',
        target: 'search',
        environment: 'staging',
        from: (NOW - 90 * DAY_MS) * 1e6,
        to: NOW * 1e6,
    });
});

test('the R5 form posts a submitted or verified record', async ({ page }) => {
    const posted = await mockApi(page);
    await page.goto('/reports');
    await pickTemplate(page, 'R5 · DR exercise report');
    const record = page.getByLabel('record', { exact: true });
    // The draft rec-2 is no DR exercise the API reports on.
    await expect(record.locator('option')).toHaveText([
        /^exercise rec-1 · failover · \d{4}-\d{2}-\d{2}$/,
        /^exercise rec-3 · failover · \d{4}-\d{2}-\d{2}$/,
    ]);
    await record.selectOption('rec-3');
    await generate(page);

    await expect.poll(() => posted).toHaveLength(1);
    expect(posted[0]).toEqual({ type: 'dr-exercise', record_id: 'rec-3' });
});
