import { render } from 'svelte/server';
import { describe, expect, it } from 'vitest';

import { emptyForm } from '../manual';
import ManualTesterFields from './ManualTesterFields.svelte';
import ManualTlptFields from './ManualTlptFields.svelte';

describe('the tester of a record', () => {
    it('takes a name and whether the tester is internal or external', () => {
        const html = render(ManualTesterFields, { props: { form: emptyForm() } }).body;
        expect(html).toContain('Tester');
        expect(html).toContain('Tester kind');
        expect(html).toContain('<option value="internal"');
        expect(html).toContain('<option value="external"');
    });
});

describe('the TLPT block of a record', () => {
    it('takes the attestation, its date and the test summary report', () => {
        const html = render(ManualTlptFields, { props: { form: emptyForm() } }).body;
        for (const label of [
            'Attestation reference',
            'Attestation date',
            'Test summary report reference',
        ]) {
            expect(html).toContain(label);
        }
        expect(html).toContain('The remediation');
    });
});
