import { expect, test } from '@playwright/test';

// One rule for how long a run took. The Runs list and the header of a run
// show the same two numbers: the run time is ended minus started, and the
// time the run waited before it started has a column of its own. The run
// was started with a var, so the run page also reads its planned blast
// radius.

const me = {
    auth_required: false,
    authenticated: true,
    username: null,
    role: null,
    env_scopes: [],
};

const QUEUED = 1_700_000_000e9;
const row = {
    id: 'run-1',
    registry_id: 'reg-1',
    state: 'passed',
    // As the API sends it: a JSON object, not a string of JSON.
    params_json: { region: 'eu-north' },
    experiment_id: 'exp-1',
    rollback_status: null,
    error: null,
    queued_at_ns: QUEUED,
    started_at_ns: QUEUED + 90e9,
    ended_at_ns: QUEUED + 102.5e9,
    definition_name: 'pg-failover',
};

// The plan of the definition with the var of the run; any other vars give
// no plan.
const scope = {
    blast_radius: 'one region',
    actions: [
        {
            step: 'pause db',
            provider: 'docker',
            action: 'pause',
            targets: { container: 'pg-eu-north' },
        },
    ],
    guards: [],
    max_concurrent_faults: null,
};

async function mockApi(page: import('@playwright/test').Page) {
    await page.route('**/api/**', async (route) => {
        const path = new URL(route.request().url()).pathname;
        let body: unknown = {};
        if (path === '/api/me') body = me;
        else if (path === '/api/runs') body = { count: 1, runs: [row] };
        else if (path === '/api/runs/run-1')
            body = { run: row, audit: [], approval: { request: null, decisions: [] } };
        else if (path === '/api/experiments/exp-1')
            body = { experiment: null, spans: [], logs: [], metrics: [] };
        else if (path === '/api/runs/dry-run') {
            const vars = route.request().postDataJSON()?.vars;
            body =
                JSON.stringify(vars) === JSON.stringify(row.params_json)
                    ? { valid: true, registry_id: 'reg-1', plan: { scope } }
                    : { valid: false, error: 'unexpected vars' };
        }
        await route.fulfill({
            status: path === '/api/runs/run-1/impact' ? 404 : 200,
            contentType: 'application/json',
            body: JSON.stringify(body),
        });
    });
}

test('the Runs list shows the run time and the wait in their own columns', async ({ page }) => {
    await mockApi(page);
    await page.goto('/runs');

    const heads = page.locator('table.data thead th');
    await expect(heads).toContainText(['Waited', 'Run time']);
    const cells = page.locator('table.data tbody tr').first().locator('td');
    const names = await heads.allTextContents();
    await expect(cells.nth(names.indexOf('Waited'))).toHaveText('1m 30s');
    await expect(cells.nth(names.indexOf('Run time'))).toHaveText('12.5s');
});

test('the header of a run shows the same run time and wait as the list', async ({ page }) => {
    await mockApi(page);
    await page.goto('/runs/run-1');

    const panel = (label: string) =>
        page.locator('.panel.meta').filter({ has: page.getByText(label, { exact: true }) });
    await expect(panel('Waited').locator('b')).toHaveText('1m 30s');
    await expect(panel('Run time').locator('b')).toHaveText('12.5s');
});

test('the run page shows the planned blast radius of a run started with a var', async ({
    page,
}) => {
    await mockApi(page);
    await page.goto('/runs/run-1');

    const planned = page.getByTestId('run-scope');
    await expect(planned).toContainText('Planned blast radius');
    await expect(planned).toContainText('one region');
    await expect(planned).toContainText('container: pg-eu-north');
});
