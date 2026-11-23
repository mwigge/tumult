import { render } from 'svelte/server';
import { describe, expect, it } from 'vitest';

import type { ManualDetail, ManualExperiment } from '../types';
import ManualExerciseView from './ManualExerciseView.svelte';

const T = Date.UTC(2026, 10, 20, 10) * 1e6;

const experiment = {
    id: 'rec-1',
    exercise_type: 'failover',
    target_system: 'payments-db',
    target_environment: 'production',
    recovery_time_s: 600,
    rto_target_s: 900,
    rpo_target_s: 60,
    rpo_measured_s: 120,
    data_verified: true,
    participants: [
        { name: 'alice', role: 'DR lead' },
        { name: 'dave', role: null },
    ],
    action_items: ['Update the runbook'],
    plan_id: 'plan-1',
} as unknown as ManualExperiment;

const detail: ManualDetail = {
    experiment,
    audit: [],
    attachments: [],
    steps: [
        {
            ordinal: 1,
            title: 'Stop the primary',
            actor: 'alice',
            started_at_ns: T,
            ended_at_ns: T + 60e9,
            result: 'done',
            note: null,
        },
        { ordinal: 2, title: 'Promote the standby', result: 'failed', note: 'slow' },
    ],
    actions: [
        {
            ordinal: 1,
            title: 'Raise the pool size',
            owner: 'dave',
            due_at_ns: T,
            ticket: 'OPS-7',
            status: 'open',
        },
    ],
    links: [{ ordinal: 1, kind: 'function', linked_id: 'payments', label: 'Payments' }],
};

const shown = (d: ManualDetail, planName = '') =>
    render(ManualExerciseView, { props: { detail: d, planName } }).body;

describe('the DR detail of a record', () => {
    it('judges the recovery time and the recovery point against their targets', () => {
        const html = shown(detail);
        expect(html).toMatch(/RTO[\s\S]*900 s[\s\S]*600 s[\s\S]*met/);
        expect(html).toMatch(/RPO[\s\S]*60 s[\s\S]*120 s[\s\S]*missed/);
        expect(html).toContain('complete');
    });

    it('shows the steps in order with their results', () => {
        const html = shown(detail);
        const first = html.indexOf('Stop the primary');
        const second = html.indexOf('Promote the standby');
        expect(first).toBeGreaterThan(-1);
        expect(second).toBeGreaterThan(first);
        expect(html).toContain('badge ok');
        expect(html).toContain('badge fail');
        expect(html).toContain('1m 0s');
    });

    it('shows the participants, the actions with owners, the links and older actions', () => {
        const html = shown(detail, 'payments-db failover');
        expect(html).toContain('DR lead');
        expect(html).toContain('Raise the pool size');
        expect(html).toContain('dave');
        expect(html).toContain('OPS-7');
        expect(html).toContain('payments');
        expect(html).toContain('Update the runbook');
        expect(html).toContain('payments-db failover');
    });

    it('says what was not recorded', () => {
        const bare = {
            experiment: {
                ...experiment,
                rto_target_s: null,
                rpo_measured_s: null,
                data_verified: null,
                participants: null,
                action_items: [],
            },
            audit: [],
            attachments: [],
        } as ManualDetail;
        const html = shown(bare);
        expect(html).toContain('not checked');
        expect(html).toContain('No steps recorded.');
        expect(html).not.toContain('missed');
    });
});
