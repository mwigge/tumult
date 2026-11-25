import { expect, test } from '@playwright/test';

// The GameDay page launches its campaign into the environment the operator
// types, and finds the campaign by asking the run list for the runs of its
// GameDay, however many other runs came after it. The API is mocked.

const me = {
    auth_required: false,
    authenticated: true,
    username: null,
    role: null,
    env_scopes: [],
};

const day = {
    id: 'gd-1',
    title: 'smoke campaign',
    description: null,
    tags: [],
    regulatory: null,
    scoring: { pass_threshold: 0.75, mttr_target_s: 30, recovery_required: true },
    experiments: [
        { path: 'a.toon', compliance_maps: [], registry_id: 'reg-a', name: 'step A' },
        { path: 'b.toon', compliance_maps: [], registry_id: 'reg-b', name: 'step B' },
    ],
    registered_at_ns: 1,
    registered_by: null,
};

function run(id: string, fields: Record<string, unknown>) {
    return {
        id,
        registry_id: 'reg-other',
        state: 'passed',
        gameday_id: null,
        params_json: null,
        experiment_id: null,
        rollback_status: null,
        error: null,
        queued_at_ns: 1,
        started_at_ns: null,
        ended_at_ns: null,
        definition_name: null,
        ...fields,
    };
}

const parent = run('camp-1', {
    registry_id: 'gd-1',
    state: 'running',
    params_json: { env: 'staging' },
    queued_at_ns: 10,
});
const child = run('child-1', { registry_id: 'reg-a', gameday_id: 'camp-1', queued_at_ns: 11 });
// The newest runs of the daemon: 50 runs of another experiment, none of
// them the campaign.
const newest = Array.from({ length: 50 }, (_, i) => run(`other-${i}`, { queued_at_ns: 100 + i }));

type Launch = { status: number; body: unknown };

/** Mocks `/api` and returns the run list queries and the launch bodies it saw. */
async function mockApi(
    page: import('@playwright/test').Page,
    opts: { campaigns: unknown[]; launch?: Launch },
) {
    const seen = { lists: [] as URLSearchParams[], launches: [] as unknown[] };
    let campaigns = opts.campaigns;
    await page.route('**/api/**', async (route) => {
        const url = new URL(route.request().url());
        const path = url.pathname;
        let status = 200;
        let body: unknown = {};
        if (path === '/api/me') body = me;
        else if (path === '/api/gamedays/gd-1') body = day;
        else if (path === '/api/gamedays/gd-1/runs' && route.request().method() === 'POST') {
            seen.launches.push(route.request().postDataJSON());
            const answer = opts.launch ?? {
                status: 202,
                body: { run_id: 'camp-1', state: 'queued', steps: 2 },
            };
            if (answer.status === 202) campaigns = [parent];
            ({ status, body } = answer);
        } else if (path === '/api/runs/camp-1')
            body = { run: parent, audit: [], approval: { request: null, decisions: [] } };
        else if (path === '/api/runs') {
            seen.lists.push(url.searchParams);
            const runs = url.searchParams.get('gameday_id')
                ? [child]
                : url.searchParams.get('registry_id') === 'gd-1'
                  ? campaigns
                  : newest;
            body = { count: runs.length, runs };
        }
        await route.fulfill({
            status,
            contentType: 'application/json',
            body: JSON.stringify(body),
        });
    });
    return seen;
}

test('the launch runs the campaign in the environment typed into the field', async ({ page }) => {
    const seen = await mockApi(page, { campaigns: [] });
    await page.goto('/gamedays/gd-1');

    const launch = page.getByRole('button', { name: 'Run GameDay' });
    await expect(launch).toBeDisabled();
    await page.getByLabel('Environment').fill('staging');
    await expect(page.getByText('The campaign runs in staging.')).toBeVisible();
    await launch.click();

    // The page follows the campaign of the answer to the runs of its steps.
    await expect(page.locator('.campaign')).toContainText('camp-1');
    await expect(page.locator('a[href="/runs/child-1"]')).toBeVisible();
    expect(seen.launches).toEqual([{ env: 'staging' }]);
    // It never looks for the campaign among the newest runs.
    for (const query of seen.lists) {
        expect(query.has('registry_id') || query.has('gameday_id'), String(query)).toBe(true);
    }
});

test('the head says that the MTTR target holds for a run with no RTO target', async ({ page }) => {
    await mockApi(page, { campaigns: [] });
    await page.goto('/gamedays/gd-1');

    // A run that sets its own rto_target_s is judged against that.
    await expect(page.locator('.page-head .sub')).toHaveText(
        '2 experiments · pass threshold 0.75 · MTTR target 30s for a run with no RTO target',
    );
});

test('the page finds its campaign by asking for the runs of its GameDay', async ({ page }) => {
    const seen = await mockApi(page, { campaigns: [parent] });
    await page.goto('/gamedays/gd-1');

    await expect(page.locator('.campaign')).toContainText('camp-1');
    await expect(page.locator('a[href="/runs/child-1"]')).toBeVisible();
    expect(seen.lists.some((query) => query.get('registry_id') === 'gd-1')).toBe(true);
});

test('a launch the server refuses shows the error with the environment', async ({ page }) => {
    await mockApi(page, {
        campaigns: [],
        launch: {
            status: 403,
            body: { error: 'execution is not bound to this environment and target' },
        },
    });
    await page.goto('/gamedays/gd-1');

    await page.getByLabel('Environment').fill('prod');
    await page.getByRole('button', { name: 'Run GameDay' }).click();

    await expect(page.locator('.state.error')).toHaveText(
        'Not allowed to run in prod: execution is not bound to this environment and target',
    );
});
