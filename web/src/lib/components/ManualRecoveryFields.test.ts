import { render } from 'svelte/server';
import { describe, expect, it } from 'vitest';

import { emptyForm } from '../manual';
import type { ManualPlan } from '../types';
import ManualPlanPicker from './ManualPlanPicker.svelte';
import ManualRecoveryFields from './ManualRecoveryFields.svelte';

const plan = (id: string, over: Partial<ManualPlan> = {}): ManualPlan =>
    ({
        id,
        name: `plan ${id}`,
        exercise_type: 'failover',
        target_system: 'payments-db',
        target_environment: 'production',
        status: 'planned',
        due_at_ns: Date.UTC(2026, 11, 1) * 1e6,
        ...over,
    }) as ManualPlan;

describe('the recovery fields of a record', () => {
    it('take the planned and the actual RTO and RPO, and whether the data was checked', () => {
        const html = render(ManualRecoveryFields, { props: { form: emptyForm() } }).body;
        for (const label of [
            'Planned RTO (s)',
            'Actual recovery time (s)',
            'Planned RPO (s)',
            'Actual RPO (s)',
            'Data verified',
        ]) {
            expect(html).toContain(label);
        }
        expect(html).toContain('loss or corruption found');
    });
});

describe('the plan a record fulfils', () => {
    it('is picked from the planned plans of the same type and target', () => {
        const form = { ...emptyForm(), exercise_type: 'failover', target_system: 'payments-db' };
        const plans = [
            plan('fits'),
            plan('restore', { exercise_type: 'restore' }),
            plan('ledger', { target_system: 'ledger' }),
        ];
        const html = render(ManualPlanPicker, { props: { form, plans } }).body;
        expect(html).toContain('Plan it fulfils');
        expect(html).toContain('value="fits"');
        expect(html).not.toContain('value="restore"');
        expect(html).not.toContain('value="ledger"');
    });

    it('keeps the plan of a draft that no longer fits', () => {
        const form = { ...emptyForm(), exercise_type: 'failover', plan_id: 'gone' };
        const html = render(ManualPlanPicker, { props: { form, plans: [] } }).body;
        expect(html).toContain('value="gone"');
    });
});
