import { expect, test } from '@playwright/test';

const me = {
    auth_required: true,
    authenticated: true,
    username: 'operator',
    role: 'operator',
    env_scopes: [],
};
const definition = {
    id: 'reg-demo',
    name: 'demo',
    content_hash: 'abcd',
    registered_at_ns: 1,
    registered_by: 'operator',
    definition_toon: 'title: demo\nmethod[1]:\n  - name: ${host}\n',
};
const plan = {
    title: 'demo',
    method: [],
    rollbacks: [],
    guards: [],
    controls: [],
    tags: [],
    scope: { blast_radius: null, actions: [], guards: [], max_concurrent_faults: null },
};

test('a delayed preview cannot arm Start for changed parameters', async ({ page }) => {
    let resolvePreview!: () => void;
    const delayed = new Promise<void>((resolve) => {
        resolvePreview = resolve;
    });
    let previewStarted!: () => void;
    const requested = new Promise<void>((resolve) => {
        previewStarted = resolve;
    });
    await page.route('**/api/**', async (route) => {
        const path = new URL(route.request().url()).pathname;
        let body: unknown = {};
        if (path === '/api/me') body = me;
        if (path === '/api/registry') body = { definitions: [definition] };
        if (path === '/api/registry/reg-demo') body = { definition };
        if (path === '/api/runs/dry-run') {
            previewStarted();
            await delayed;
            body = { valid: true, registry_id: definition.id, execution_hash: '1234', plan };
        }
        await route.fulfill({ json: body });
    });
    await page.goto('/runs/new?registry_id=reg-demo');
    await page.getByLabel('Environment', { exact: true }).fill('staging');
    await page.getByLabel('host', { exact: true }).fill('server-a');
    await page.getByRole('button', { name: 'Dry run', exact: true }).click();
    await requested;
    await page.getByLabel('host', { exact: true }).fill('server-b');
    resolvePreview();
    await expect(page.getByRole('button', { name: 'Dry run', exact: true })).toBeEnabled();
    await expect(page.getByRole('button', { name: /Start/ })).toBeDisabled();
});

test('a saved draft can be reopened, edited, and submitted without duplication', async ({
    page,
}) => {
    let record: Record<string, unknown> | null = null;
    let creates = 0;
    let updates = 0;
    let submissions = 0;
    await page.route('**/api/**', async (route) => {
        const path = new URL(route.request().url()).pathname;
        const method = route.request().method();
        let body: unknown = {};
        if (path === '/api/me') body = me;
        if (path === '/api/manual/experiments' && method === 'POST') {
            creates++;
            record = {
                ...route.request().postDataJSON(),
                id: 'manual-1',
                status: 'draft',
                entered_by: me.username,
                content_hash: 'abcd',
                reviewed_by: null,
            };
            body = { id: 'manual-1' };
        } else if (path === '/api/manual/experiments') body = { records: record ? [record] : [] };
        if (path === '/api/manual/experiments/manual-1' && method === 'PUT') {
            updates++;
            record = { ...record, ...route.request().postDataJSON() };
            body = { ok: true };
        } else if (path === '/api/manual/experiments/manual-1')
            body = { experiment: record, audit: [], attachments: [] };
        if (path.endsWith('/submit')) {
            submissions++;
            if (submissions === 1) {
                await route.fulfill({
                    status: 503,
                    json: { error: 'temporary submission outage' },
                });
                return;
            }
            record = { ...record, status: 'submitted' };
            body = { ok: true };
        }
        await route.fulfill({ json: body });
    });
    await page.goto('/manual');
    await page.getByLabel('Experiment name').fill('Draft journey');
    await page.getByLabel('Executed at').fill('2026-10-08T12:00');
    await page.getByLabel('Hypothesis', { exact: true }).first().fill('Healthy');
    await page.getByLabel('Method', { exact: true }).fill('Original method');
    await page.getByLabel('Attestation text', { exact: true }).fill('I attest this is accurate');
    await page.getByRole('button', { name: 'Save draft', exact: true }).click();
    await expect(page.getByText(/draft manual-1 saved/)).toBeVisible();
    await page.getByRole('button', { name: 'All records', exact: true }).click();
    await page.getByRole('button', { name: 'Edit draft', exact: true }).click();
    await expect(page.getByLabel('Method', { exact: true })).toHaveValue('Original method');
    await page.getByLabel('Method', { exact: true }).fill('Reviewed method');
    await page.getByRole('button', { name: 'Save & submit', exact: true }).click();
    await expect(page.getByText(/temporary submission outage/)).toBeVisible();
    await page.getByRole('button', { name: 'Save & submit', exact: true }).click();
    await expect(page.getByText(/submitted for verification/)).toBeVisible();
    expect(creates).toBe(1);
    expect(updates).toBeGreaterThan(0);
    expect((record as Record<string, unknown> | null)?.status).toBe('submitted');
});

test('a viewer can read reports but cannot start report generation', async ({ page }) => {
    await page.route('**/api/**', async (route) => {
        const path = new URL(route.request().url()).pathname;
        const body =
            path === '/api/me'
                ? { ...me, role: 'viewer' }
                : path === '/api/experiments'
                  ? { experiments: [] }
                  : path === '/api/metrics'
                    ? { metrics: [] }
                    : { reports: [] };
        await route.fulfill({ json: body });
    });
    await page.goto('/reports');
    await expect(page.getByRole('button', { name: 'Generate', exact: true }).first()).toBeVisible();
    for (const button of await page.getByRole('button', { name: 'Generate', exact: true }).all()) {
        await expect(button).toBeDisabled();
    }
    await expect(
        page.getByText('Generating reports requires the operator role or above.'),
    ).toBeVisible();
});

test('Start submits the preview fingerprint and explicit destination', async ({ page }) => {
    let submitted: Record<string, unknown> | null = null;
    await page.route('**/api/**', async (route) => {
        const path = new URL(route.request().url()).pathname;
        let body: unknown = {};
        if (path === '/api/me') body = me;
        if (path === '/api/registry') body = { definitions: [definition] };
        if (path === '/api/registry/reg-demo') body = { definition };
        if (path === '/api/runs/dry-run')
            body = {
                valid: true,
                registry_id: definition.id,
                execution_hash: 'reviewed-fingerprint',
                execution_context: {
                    env: 'production',
                    target: 'cluster-a',
                    tier: 'T3',
                    error: null,
                },
                plan,
            };
        if (path === '/api/runs') {
            submitted = route.request().postDataJSON();
            await route.fulfill({
                status: 409,
                json: { error: 'execution inputs changed since preview' },
            });
            return;
        }
        await route.fulfill({ json: body });
    });
    await page.goto('/runs/new?registry_id=reg-demo');
    await page.getByLabel('Environment', { exact: true }).fill('production');
    await page.getByLabel('Target', { exact: true }).fill('cluster-a');
    await page.getByLabel('host', { exact: true }).fill('server-a');
    await page.getByRole('button', { name: 'Dry run', exact: true }).click();
    await expect(page.getByRole('button', { name: /Start/ })).toBeEnabled();
    await expect(page.getByText('reviewed-fingerprint', { exact: true })).toHaveCount(0);
    await page.getByRole('button', { name: /Start/ }).click();
    await expect(page.getByText(/execution inputs changed since preview/)).toBeVisible();
    expect(submitted).toEqual({
        registry_id: 'reg-demo',
        vars: { host: 'server-a' },
        env: 'production',
        target: 'cluster-a',
        execution_hash: 'reviewed-fingerprint',
    });
});

test('an unbound scoped preview explains the binding requirement and cannot start', async ({
    page,
}) => {
    await page.route('**/api/**', async (route) => {
        const path = new URL(route.request().url()).pathname;
        let body: unknown = {};
        if (path === '/api/me') body = { ...me, env_scopes: ['staging'] };
        if (path === '/api/registry') body = { definitions: [definition] };
        if (path === '/api/registry/reg-demo') body = { definition };
        if (path === '/api/runs/dry-run')
            body = {
                valid: true,
                registry_id: definition.id,
                execution_hash: 'needs-administrator-review',
                plan,
                execution_context: {
                    env: 'staging',
                    error: 'Ask the administrator to bind this execution fingerprint.',
                },
            };
        await route.fulfill({ json: body });
    });
    await page.goto('/runs/new?registry_id=reg-demo');
    await page.getByLabel('Environment', { exact: true }).fill('staging');
    await page.getByLabel('host', { exact: true }).fill('server-a');
    await page.getByRole('button', { name: 'Dry run', exact: true }).click();
    await expect(
        page.getByText('Ask the administrator to bind this execution fingerprint.'),
    ).toBeVisible();
    await expect(page.getByRole('button', { name: /Start/ })).toBeDisabled();
});

test('a scoped evidence author gets a clear environment requirement before saving', async ({
    page,
}) => {
    let creates = 0;
    await page.route('**/api/**', async (route) => {
        const path = new URL(route.request().url()).pathname;
        let body: unknown = {};
        if (path === '/api/me') body = { ...me, env_scopes: ['staging', 'development'] };
        if (path === '/api/manual/experiments') {
            creates++;
            body = { id: 'scoped-manual' };
        }
        await route.fulfill({ json: body });
    });
    await page.goto('/manual');
    await page.getByLabel('Experiment name').fill('Scoped draft');
    await page.getByLabel('Executed at').fill('2026-10-08T12:00');
    await page.getByLabel('Hypothesis', { exact: true }).first().fill('Healthy');
    await page.getByLabel('Method', { exact: true }).fill('Manual check');
    await page.getByLabel('Attestation text', { exact: true }).fill('I attest');
    await page.getByRole('button', { name: 'Save draft', exact: true }).click();
    await expect(
        page.getByText('Choose a target environment allowed by your account: staging, development'),
    ).toBeVisible();
    expect(creates).toBe(0);
    await page.getByLabel('Target environment', { exact: true }).fill('staging');
    await page.getByRole('button', { name: 'Save draft', exact: true }).click();
    await expect(page.getByText(/draft scoped-manual saved/)).toBeVisible();
    expect(creates).toBe(1);
});
