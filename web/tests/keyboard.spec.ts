import { expect, test } from '@playwright/test';

import type { LogRow, ScoreTree, TraceRow } from '../src/lib/types';
import { bodies } from './chart-theme-data';
import { show, tabTo, targetOf } from './keyboard';
import { manualApi } from './manual-data';
import { NOW_MS } from './style-snapshot-pages';

// A keyboard reaches and opens every row that a click opens. Tab moves to
// the link or button in the cell that names the row, and Enter opens it.
// A row that opens in place says so with aria-expanded. A click on the
// rest of the row still opens it, for a mouse.

test.use({ locale: 'en-US', timezoneId: 'UTC' });

const HOUR_NS = 3_600_000_000_000;
const NOW_NS = NOW_MS * 1_000_000;

const trace: TraceRow = {
    trace_id: 'trace-1',
    started_ns: NOW_NS - HOUR_NS,
    duration_ns: 120_000_000,
    span_count: 4,
    error_count: 0,
    root_name: 'resilience.experiment',
    service_name: 'tumult',
    experiment_id: null,
    experiment_name: null,
    status: 'completed',
};

const log: LogRow = {
    ts_ns: NOW_NS - HOUR_NS,
    severity_text: 'ERROR',
    body: 'connection refused',
    trace_id: 't-1',
    span_id: 'action',
    log_attrs: { svc: 'api' },
};

// The team commerce with one service: the Scores page lists the members
// of a node it drilled into.
const company = bodies['/api/scores/tree'] as ScoreTree;
const commerce: ScoreTree = {
    ...company,
    path: 'commerce',
    name: 'commerce',
    children: [{ ...company.children[0], path: 'commerce/checkout', name: 'checkout' }],
};

/** Pages whose first row links to the page of the row. */
const LINKS = [
    { target: targetOf('runs'), link: 'pg-failover', opens: /\/runs\/run-1$/ },
    { target: targetOf('experiments'), link: 'pg-failover', opens: /\/experiments\/exp-1$/ },
    { target: targetOf('gamedays'), link: 'smoke campaign', opens: /\/gamedays\/gd-1$/ },
    {
        target: targetOf('traces', { api: { '/api/traces': { count: 1, traces: [trace] } } }),
        link: 'resilience.experiment',
        opens: /\/traces\/trace-1$/,
    },
    {
        target: targetOf('scores', {
            path: '/scores?node=commerce',
            api: { '/api/scores/tree': commerce },
        }),
        link: 'checkout',
        opens: /\/scores\?node=commerce%2Fcheckout$/,
    },
    {
        target: targetOf('author'),
        link: 'add-latency',
        opens: /\/author\/new\?plugin=tumult-network&action=add-latency$/,
    },
];

for (const { target, link, opens } of LINKS) {
    test(`on ${target.path}, Tab reaches the link of the first row and Enter opens the row`, async ({
        page,
    }) => {
        await show(page, target);
        const first = page.getByRole('table').getByRole('link', { name: link, exact: true });
        await tabTo(page, first.first());
        await page.keyboard.press('Enter');
        await expect(page).toHaveURL(opens);
    });
}

test('on the new run page, Tab reaches the first definition and Enter picks it', async ({
    page,
}) => {
    await show(page, targetOf('runs-new', { path: '/runs/new' }));
    const pick = page.getByRole('button', { name: 'demo', exact: true });
    await expect(pick).toHaveAttribute('aria-pressed', 'false');
    await tabTo(page, pick);
    await page.keyboard.press('Enter');
    await expect(pick).toHaveAttribute('aria-pressed', 'true');
    await expect(
        page.getByRole('heading', { name: '2 · Execution context and parameters' }),
    ).toBeVisible();
});

test('on the reports page, Tab reaches the first document and Enter shows its preview', async ({
    page,
}) => {
    await show(page, targetOf('reports'));
    await tabTo(page, page.getByRole('button', { name: 'doc-1', exact: true }));
    await page.keyboard.press('Enter');
    await expect(page.locator('iframe[title="report preview"]')).toBeVisible();
});

/** Pages whose rows open in place, and the button that names the first row. */
const EXPANDS = [
    {
        target: targetOf('logs', { api: { '/api/logs': { count: 1, logs: [log] } } }),
        button: '1h ago',
    },
    { target: targetOf('users'), button: 'admin' },
    { target: targetOf('approvals'), button: 'pg-failover' },
    {
        target: targetOf('manual', { path: '/manual?tab=plans', api: manualApi }),
        button: 'payments failover',
    },
    {
        target: targetOf('manual', { path: '/manual?tab=queue', api: manualApi }),
        button: 'exercise rec-1',
    },
    {
        target: targetOf('manual', { path: '/manual?tab=records', api: manualApi }),
        button: 'exercise rec-1',
    },
];

for (const { target, button } of EXPANDS) {
    test(`on ${target.path}, Enter on the button of the first row opens and closes it, and aria-expanded says which`, async ({
        page,
    }) => {
        await show(page, target);
        const toggle = page.getByRole('button', { name: button, exact: true });
        await expect(toggle).toHaveAttribute('aria-expanded', 'false');
        await tabTo(page, toggle);
        await page.keyboard.press('Enter');
        await expect(toggle).toHaveAttribute('aria-expanded', 'true');
        await expect(page.locator('tr.detail-row')).toBeVisible();
        await expect(toggle).toBeFocused();
        await page.keyboard.press('Enter');
        await expect(toggle).toHaveAttribute('aria-expanded', 'false');
        await expect(page.locator('tr.detail-row')).toHaveCount(0);
    });
}

test('a click on the rest of a row still opens it', async ({ page }) => {
    await show(page, targetOf('runs'));
    await page.getByRole('row').filter({ hasText: 'run-1' }).getByText('pg-failover').click();
    await expect(page).toHaveURL(/\/runs\/run-1$/);
});
