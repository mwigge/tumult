import { render } from 'svelte/server';
import { describe, expect, it } from 'vitest';

import type { ManualDetail, ManualExperiment } from '../types';
import ManualAuditList from './ManualAuditList.svelte';
import ManualPlansTab from './ManualPlansTab.svelte';
import ManualRecordDetail from './ManualRecordDetail.svelte';

const T = Date.UTC(2026, 10, 20, 10) * 1e6;

const detail: ManualDetail = {
    experiment: {
        id: 'rec-1',
        status: 'submitted',
        exercise_type: 'failover',
        content_hash: 'abcdef0123456789abcdef',
        attestation: 'I attest',
        action_items: [],
    } as unknown as ManualExperiment,
    audit: [
        { id: 'a1', action: 'create', changed_by: 'alice', changed_at_ns: T },
        { id: 'a2', action: 'submit', changed_by: 'alice', changed_at_ns: T + 1e9 },
    ] as unknown as ManualDetail['audit'],
    attachments: [],
    signoffs: [],
    signoffs_required: 2,
    signoffs_needed: 2,
};

describe('an opened record', () => {
    it('shows its sign-offs, its audit trail, its seal, its evidence and the verify', () => {
        const html = render(ManualRecordDetail, {
            props: { detail, actor: 'bob', canReview: true, canWrite: true },
        }).body;
        expect(html).toContain('0 of 2 approvals. 2 more sign-offs needed.');
        expect(html).toContain('content hash abcdef0123456789');
        expect(html).toContain('No evidence attached yet.');
        expect(html).toContain('>Verify audit trail<');
        expect(html).toContain('I attest');
    });
});

describe('the audit trail of a record or a plan', () => {
    it('lists each row in the order the API gives, with who made it', () => {
        const html = render(ManualAuditList, { props: { rows: detail.audit } }).body;
        expect(html.indexOf('create')).toBeLessThan(html.indexOf('submit'));
        expect(html).toContain('by alice');
    });
});

describe('the plans tab', () => {
    it('offers the plan form to a writer, and says who can plan to a reader', () => {
        const writer = render(ManualPlansTab, { props: { actor: 'alice', canWrite: true } }).body;
        expect(writer).toContain('Plan an exercise');
        expect(writer).toContain('Save plan');
        const reader = render(ManualPlansTab, { props: { actor: '', canWrite: false } }).body;
        expect(reader).not.toContain('Save plan');
        expect(reader).toContain('An operator can plan an exercise.');
    });
});
