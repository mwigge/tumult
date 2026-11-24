import { render } from 'svelte/server';
import { describe, expect, it } from 'vitest';

import type { ManualSignOffRow } from '../types';
import ManualSignOff from './ManualSignOff.svelte';

const approval: ManualSignOffRow = {
    id: 'so-1',
    role: 'reviewer',
    signed_by: 'bob',
    signed_at_ns: Date.UTC(2026, 10, 21) * 1e6,
    decision: 'approved',
    note: 'steps match the log',
    signed_hash: 'abc',
};

const shown = (props: Record<string, unknown>) =>
    render(ManualSignOff, {
        props: {
            experimentId: 'rec-1',
            status: 'submitted',
            signoffs: [approval],
            required: 2,
            needed: 1,
            actor: 'carol',
            canReview: true,
            ...props,
        },
    }).body;

describe('the sign-off of a record', () => {
    it('says how many sign-offs are still needed, and takes one with a note', () => {
        const html = shown({});
        expect(html).toContain('1 of 2 approvals. 1 more sign-off needed.');
        expect(html).toContain('Sign-off note');
        expect(html).toContain('>Approve<');
        expect(html).toContain('>Reject<');
    });

    it('lists each sign-off with who gave it and the note', () => {
        const html = shown({});
        expect(html).toContain('approved');
        expect(html).toContain('bob');
        expect(html).toContain('steps match the log');
    });

    it('takes no sign-off once the record is verified', () => {
        const html = shown({ status: 'verified', signoffs: [approval, approval], needed: 0 });
        expect(html).toContain('Verified with 2 of 2 approvals.');
        expect(html).not.toContain('>Approve<');
    });

    it('takes no sign-off from a role that cannot give one', () => {
        const html = shown({ canReview: false });
        expect(html).not.toContain('>Approve<');
        expect(html).toContain('An approver signs off a record.');
    });

    it('takes no sign-off before the record is submitted', () => {
        const html = shown({ status: 'draft', signoffs: [], needed: 2 });
        expect(html).not.toContain('>Approve<');
        expect(html).toContain('A record is signed off once it is submitted.');
    });
});
