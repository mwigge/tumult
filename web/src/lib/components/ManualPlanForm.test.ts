import { render } from 'svelte/server';
import { describe, expect, it } from 'vitest';

import { emptyPlanForm } from '../manual';
import ManualPlanForm from './ManualPlanForm.svelte';

const form = (editing: boolean) =>
    render(ManualPlanForm, {
        props: { form: emptyPlanForm(), editing, busy: false, message: null },
    }).body;

describe('the plan form', () => {
    it('has a label for every field of a plan', () => {
        const html = form(false);
        for (const label of [
            'Plan name',
            'Exercise type',
            'Target system',
            'Target environment',
            'Function',
            'Scope',
            'Owner',
            'Planned date',
            'Due date',
            'Repeat every (days)',
            'Planned RTO (s)',
            'Planned RPO (s)',
        ]) {
            expect(html, label).toContain(`${label}`);
        }
        expect(html).toContain('<option value="restore"');
        expect(html).toContain('<option value="backup"');
    });

    it('saves a new plan, or the changes of the plan being edited', () => {
        expect(form(false)).toContain('Save plan');
        expect(form(false)).not.toContain('Stop editing');
        expect(form(true)).toContain('Save changes');
        expect(form(true)).toContain('Stop editing');
    });
});
