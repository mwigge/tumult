import { expect, test } from '@playwright/test';
import type { Page } from '@playwright/test';

// A run that sent no trace is in the lake as a journal only. The list and
// the page of the run say so, and the page does not look broken for having
// no spans.

const me = {
    auth_required: false,
    authenticated: true,
    username: null,
    role: null,
    env_scopes: [],
};

const T0 = Date.UTC(2026, 10, 15, 19, 26, 39) * 1_000_000;
const SECOND = 1_000_000_000;

function row(id: string, name: string, journalOnly: boolean) {
    return {
        id,
        name,
        started_ns: T0,
        duration_ns: 30 * SECOND,
        trace_id: journalOnly ? null : `trace-${id}`,
        target_system: 'payments-api',
        target_technology: 'postgresql',
        target_environment: 'staging',
        status: 'deviated',
        deviations: null,
        duration_ms: null,
        journal_only: journalOnly,
        faults: null,
        origin: 'automated',
        review_status: null,
    };
}

const fromTheCli = row('cli-1', 'payments-replica-loss', true);
const traced = row('exp-1', 'pg-failover', false);

function detail(experiment: ReturnType<typeof row>) {
    return { experiment, spans: [], logs: [], metrics: [] };
}

async function mock(page: Page) {
    await page.route('**/api/**', async (route) => {
        const path = new URL(route.request().url()).pathname;
        if (path === '/api/me') await route.fulfill({ json: me });
        else if (path === '/api/experiments')
            await route.fulfill({ json: { count: 2, experiments: [fromTheCli, traced] } });
        else if (path === '/api/experiments/cli-1')
            await route.fulfill({ json: detail(fromTheCli) });
        else if (path === '/api/experiments/exp-1') await route.fulfill({ json: detail(traced) });
        else if (path === '/api/dimensions')
            await route.fulfill({
                json: { outcomes: [], targets: [], faults: [], experiments: [] },
            });
        else await route.fulfill({ status: 404, json: { error: 'not found' } });
    });
}

test('the list marks the run that is a journal only', async ({ page }) => {
    await mock(page);
    await page.goto('/experiments');

    const rows = page.locator('table.data tbody tr');
    await expect(rows).toHaveCount(2);
    await expect(rows.filter({ hasText: 'payments-replica-loss' })).toContainText('journal only');
    await expect(rows.filter({ hasText: 'pg-failover' })).not.toContainText('journal only');
});

test('the run opens from the list and its page says it sent no trace', async ({ page }) => {
    await mock(page);
    await page.goto('/experiments');
    await page.locator('table.data tbody tr').filter({ hasText: 'payments-replica-loss' }).click();

    await expect(page).toHaveURL(/\/experiments\/cli-1$/);
    await expect(page.locator('.page-head')).toContainText('journal only');
    await expect(page.getByTestId('no-trace')).toContainText('This run sent no trace');
    await expect(page.locator('body')).not.toContainText('No spans recorded');
    // What the journal knows is still shown.
    await expect(page.locator('body')).toContainText('payments-api · postgresql · staging');
    await expect(page.locator('body')).toContainText('30.0s');
});

test('the page of a run with a trace is not marked', async ({ page }) => {
    await mock(page);
    await page.goto('/experiments/exp-1');

    await expect(page.locator('.page-head')).toContainText('pg-failover');
    await expect(page.locator('.page-head')).not.toContainText('journal only');
    await expect(page.getByTestId('no-trace')).toHaveCount(0);
});
