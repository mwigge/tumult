import { expect, test } from '@playwright/test';
import type { Locator, Page } from '@playwright/test';

// The span waterfall is HTML, not a chart: its grid is asserted by the
// geometry the browser lays out. Every grid line stands in the lane the
// bars are drawn in, under its own tick label, and ends with the last row.
// A row opens the span drawer: a dialog that the span names. Escape, its
// close button and the shade beside it close it, and the focus goes back
// to the row.

const me = {
    auth_required: false,
    authenticated: true,
    username: null,
    role: null,
    env_scopes: [],
};

const T0 = Date.UTC(2026, 10, 15, 19, 26, 39) * 1_000_000;
const MS = 1_000_000;

function span(id: string, parent: string | null, name: string, startMs: number, ms: number) {
    return {
        ts_ns: T0 + startMs * MS,
        trace_id: 't-1',
        span_id: id,
        parent_span_id: parent,
        span_name: name,
        span_kind: 'Internal',
        duration_ns: ms * MS,
        status_code: 'Unset',
        status_message: '',
        service_name: 'tumult',
        fault_type: null,
        fault_subtype: null,
        span_attrs: {},
    };
}

const spans = [
    span('root', null, 'resilience.experiment', 0, 5300),
    span('before', 'root', 'resilience.hypothesis.before', 0, 1),
    span('action', 'root', 'resilience.action', 2, 3000),
    span('p1', 'root', 'resilience.probe', 1000, 1),
    span('p2', 'root', 'resilience.probe', 2000, 1),
    span('after', 'root', 'resilience.hypothesis.after', 3010, 1),
    span('ingest', null, 'resilience.analytics.ingest', 5310, 17),
];

const detail = {
    experiment: {
        id: 'exp-1',
        name: 'host cpu burn',
        started_ns: T0,
        duration_ns: 3004 * MS,
        trace_id: 't-1',
        target_system: 'host',
        target_technology: null,
        target_environment: null,
        status: 'completed',
        deviations: '0',
        duration_ms: '3004',
        journal_only: false,
        faults: null,
        origin: 'automated',
        review_status: null,
    },
    spans,
    logs: [],
    metrics: [],
};

async function mock(page: Page) {
    await page.route('**/api/**', async (route) => {
        const path = new URL(route.request().url()).pathname;
        if (path === '/api/me') await route.fulfill({ json: me });
        else if (path === '/api/experiments/exp-1') await route.fulfill({ json: detail });
        else await route.fulfill({ status: 404, json: { error: 'not found' } });
    });
}

interface Box {
    x: number;
    y: number;
    width: number;
    height: number;
}

async function box(locator: Locator): Promise<Box> {
    const b = await locator.boundingBox();
    if (!b) throw new Error('not laid out');
    return b;
}

const HALF_PX = 0.5;

test('the waterfall grid stays inside the lanes and ends with the last row', async ({ page }) => {
    await mock(page);
    await page.goto('/experiments/exp-1');
    const wf = page.locator('.wf');
    const rows = wf.locator('.row');
    await expect(rows).toHaveCount(spans.length);

    const lane = await box(rows.first().locator('.lane'));
    const last = await box(rows.last());
    const lines = wf.locator('.grid i');
    const labels = wf.locator('.labels span');
    await expect(lines).toHaveCount(6);
    await expect(labels).toHaveCount(6);

    for (let i = 0; i < 6; i++) {
        const line = await box(lines.nth(i));
        const label = await box(labels.nth(i));
        const at = line.x + line.width / 2;
        // Inside the lane, never over the names or the durations.
        expect(at, `line ${i} left of the lane`).toBeGreaterThanOrEqual(lane.x - HALF_PX);
        expect(at, `line ${i} right of the lane`).toBeLessThanOrEqual(
            lane.x + lane.width + HALF_PX,
        );
        // At its share of the lane, under its own label.
        expect(Math.abs(at - (lane.x + (lane.width * i) / 5)), `line ${i} place`).toBeLessThan(1);
        expect(Math.abs(label.x + label.width / 2 - at), `label ${i} over its line`).toBeLessThan(
            1,
        );
        // Down to the last row and no further.
        expect(line.y + line.height, `line ${i} bottom`).toBeLessThanOrEqual(
            last.y + last.height + HALF_PX,
        );
        expect(line.y + line.height, `line ${i} reaches the last row`).toBeGreaterThan(last.y);
    }
});

/** The row of the span `resilience.action`: a button that opens its drawer. */
const actionRow = (page: Page) => page.locator('.wf .row').nth(2);

test('a row opens the span drawer from the keyboard, and Escape gives the focus back', async ({
    page,
}) => {
    await mock(page);
    await page.goto('/experiments/exp-1');
    const row = actionRow(page);
    await row.focus();
    await page.keyboard.press('Enter');

    // The drawer is a dialog that the span names, with the focus on its close button.
    const drawer = page.getByRole('dialog', { name: 'resilience.action' });
    await expect(drawer).toBeVisible();
    await expect(drawer.getByRole('button', { name: 'close' })).toBeFocused();

    await page.keyboard.press('Escape');
    await expect(page.locator('dialog')).toHaveCount(0);
    await expect(row).toBeFocused();

    // The row opens it again.
    await page.keyboard.press('Enter');
    await expect(drawer).toBeVisible();
});

test('the close button and the shade close the span drawer, a click in it does not', async ({
    page,
}) => {
    await mock(page);
    await page.goto('/experiments/exp-1');
    const row = actionRow(page);
    const drawer = page.getByRole('dialog', { name: 'resilience.action' });

    await row.click();
    await drawer.getByRole('button', { name: 'close' }).click();
    await expect(page.locator('dialog')).toHaveCount(0);
    await expect(row).toBeFocused();

    await row.click();
    const edge = await box(drawer);
    await page.mouse.click(edge.x + 4, edge.y + 4);
    await expect(drawer).toBeVisible();
    await page.mouse.click(edge.x / 2, edge.y + edge.height / 2);
    await expect(page.locator('dialog')).toHaveCount(0);
    await expect(row).toBeFocused();
});
