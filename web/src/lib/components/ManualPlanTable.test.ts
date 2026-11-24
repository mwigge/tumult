import { render } from 'svelte/server';
import { describe, expect, it } from 'vitest';

import type { ManualPlan } from '../types';
import ManualPlanTable from './ManualPlanTable.svelte';

const DAY = 86_400 * 1e9;
const NOW = Date.UTC(2026, 10, 23) * 1e6;

function plan(over: Partial<ManualPlan>): ManualPlan {
    return {
        id: 'plan',
        name: 'payments-db failover',
        exercise_type: 'failover',
        target_system: 'payments-db',
        target_environment: 'production',
        function_id: null,
        scope: null,
        owner: 'alice',
        planned_at_ns: null,
        due_at_ns: NOW + 30 * DAY,
        interval_days: 365,
        rto_target_s: 900,
        rpo_target_s: 60,
        status: 'planned',
        created_by: 'alice',
        created_at_ns: NOW - DAY,
        updated_by: null,
        updated_at_ns: null,
        ...over,
    };
}

/** The row of one plan in the server render. */
function row(html: string, id: string): string {
    const start = html.indexOf(`data-plan="${id}"`);
    expect(start, `a row for ${id}`).toBeGreaterThan(-1);
    return html.slice(start, html.indexOf('</tr>', start));
}

const table = (plans: ManualPlan[], canWrite = true) =>
    render(ManualPlanTable, { props: { plans, nowNs: NOW, canWrite } }).body;

describe('the table of plans', () => {
    it('marks a planned plan past its due date as overdue, and no other', () => {
        const html = table([
            plan({ id: 'late', due_at_ns: NOW - 3 * DAY }),
            plan({ id: 'soon' }),
            plan({ id: 'done', status: 'fulfilled', due_at_ns: NOW - 3 * DAY }),
        ]);
        expect(row(html, 'late')).toContain('overdue');
        expect(row(html, 'soon')).not.toContain('overdue');
        expect(row(html, 'done')).not.toContain('overdue');
        expect(row(html, 'done')).toContain('fulfilled');
    });

    it('shows the target, the owner, the interval and the planned objectives', () => {
        const html = row(table([plan({ id: 'p' })]), 'p');
        expect(html).toContain('payments-db');
        expect(html).toContain('production');
        expect(html).toContain('alice');
        expect(html).toContain('every 365 days');
        expect(html).toContain('900 s');
        expect(html).toContain('60 s');
    });

    it('offers edit and cancel for a planned plan to a writer only', () => {
        const plans = [plan({ id: 'open' }), plan({ id: 'gone', status: 'cancelled' })];
        const html = table(plans);
        expect(row(html, 'open')).toContain('aria-label="Edit plan payments-db failover"');
        expect(row(html, 'open')).toContain('aria-label="Cancel plan payments-db failover"');
        expect(row(html, 'gone')).not.toContain('Edit plan');
        expect(table(plans, false)).not.toContain('Edit plan');
    });

    it('says when there is no plan yet', () => {
        expect(table([])).toContain('No plans yet.');
    });
});
