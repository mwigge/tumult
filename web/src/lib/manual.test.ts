import { describe, expect, it } from 'vitest';

import {
    applyPlan,
    dayEndNs,
    detailToForm,
    emptyAction,
    emptyForm,
    emptyPlanForm,
    fmtBytes,
    formToRecord,
    importBody,
    isOverdue,
    localInput,
    matchingPlans,
    ofTarget,
    planPatch,
    planRequest,
    planToForm,
    recoveryVerdict,
    signoffSummary,
    targetFilterOf,
    targetQuery,
} from './manual';
import type { ManualDetail, ManualExperiment, ManualPlan } from './types';

const NS = 1_000_000;
const local = (...parts: [number, number, number, number?, number?, number?, number?]) =>
    new Date(...(parts as [number, number, number, number, number, number, number])).getTime() * NS;

function plan(over: Partial<ManualPlan> = {}): ManualPlan {
    return {
        id: 'plan-1',
        name: 'payments-db failover',
        exercise_type: 'failover',
        target_system: 'payments-db',
        target_environment: 'production',
        function_id: null,
        scope: null,
        owner: 'alice',
        planned_at_ns: null,
        due_at_ns: local(2026, 11, 1, 23, 59, 59, 999),
        interval_days: 365,
        rto_target_s: 900,
        rpo_target_s: 60,
        status: 'planned',
        created_by: 'alice',
        created_at_ns: local(2026, 10, 1),
        updated_by: null,
        updated_at_ns: null,
        ...over,
    };
}

function filledForm() {
    return {
        ...emptyForm(),
        experiment_name: 'payments-db failover',
        exercise_type: 'failover',
        executed: '2026-11-20T10:00',
        hypothesis: 'The standby takes the writes within 900 s',
        method: 'Stop the primary and promote the standby',
        attestation: 'I attest',
        target_system: 'payments-db',
        target_environment: 'production',
    };
}

describe('the record the form sends', () => {
    it('sends the actions with owners and due dates and no older action list', () => {
        const form = {
            ...filledForm(),
            actions: [
                {
                    ...emptyAction(),
                    title: 'Raise the pool size',
                    owner: 'dave',
                    due: '2026-12-01',
                    ticket: 'OPS-7',
                },
            ],
        };
        const record = formToRecord(form, 'alice');
        expect(record.actions).toEqual([
            {
                title: 'Raise the pool size',
                owner: 'dave',
                due_at_ns: local(2026, 11, 1, 23, 59, 59, 999),
                ticket: 'OPS-7',
                severity: null,
                retest: null,
            },
        ]);
        expect(record).not.toHaveProperty('action_items');
    });

    it('sends the steps in order with their times and results', () => {
        const form = {
            ...filledForm(),
            steps: [
                {
                    title: 'Stop the primary',
                    actor: 'alice',
                    started: '2026-11-20T10:00:05',
                    ended: '2026-11-20T10:01:10',
                    result: 'done',
                    note: '',
                },
                { title: 'Promote', actor: '', started: '', ended: '', result: '', note: 'slow' },
            ],
        };
        expect(formToRecord(form, 'alice').steps).toEqual([
            {
                title: 'Stop the primary',
                actor: 'alice',
                started_at_ns: local(2026, 10, 20, 10, 0, 5, 0),
                ended_at_ns: local(2026, 10, 20, 10, 1, 10, 0),
                result: 'done',
                note: null,
            },
            {
                title: 'Promote',
                actor: null,
                started_at_ns: null,
                ended_at_ns: null,
                result: null,
                note: 'slow',
            },
        ]);
    });

    it('leaves out a row with nothing in it, and sends a row with a field but no title', () => {
        const form = {
            ...filledForm(),
            participants: [
                { name: '', role: '' },
                { name: '', role: 'scribe' },
            ],
            steps: [{ title: '', actor: '', started: '', ended: '', result: '', note: '' }],
        };
        const record = formToRecord(form, 'alice');
        expect(record.participants).toEqual([{ name: '', role: 'scribe' }]);
        expect(record.steps).toEqual([]);
    });

    it('sends the recovery numbers, a zero as a zero and a blank as nothing', () => {
        const form = {
            ...filledForm(),
            rto_target_s: 900,
            // A number field gives a number: a zero must stay a zero.
            recovery_time_s: 0,
            rpo_target_s: '',
            rpo_measured_s: 12.5,
            data_verified: 'no',
            plan_id: 'plan-1',
        };
        const record = formToRecord(form, 'alice');
        expect(record).toMatchObject({
            rto_target_s: 900,
            recovery_time_s: 0,
            rpo_target_s: null,
            rpo_measured_s: 12.5,
            data_verified: false,
            plan_id: 'plan-1',
            entered_by: 'alice',
        });
        expect(formToRecord({ ...form, data_verified: 'unknown', plan_id: '' }, 'a')).toMatchObject(
            {
                data_verified: null,
                plan_id: null,
            },
        );
    });

    it('sends the severities, the tester and a retest of an action', () => {
        const form = {
            ...filledForm(),
            findings_severity: 'high',
            tester_name: ' Red Team Ltd ',
            tester_kind: 'external',
            actions: [
                {
                    ...emptyAction(),
                    title: 'Remove stale accounts',
                    owner: 'carol',
                    severity: 'critical',
                    retest_kind: 'record',
                    retest_id: ' rec-9 ',
                },
            ],
        };
        const record = formToRecord(form, 'alice');
        expect(record).toMatchObject({
            findings_severity: 'high',
            tester: { name: 'Red Team Ltd', kind: 'external' },
        });
        expect(record.actions?.[0]).toMatchObject({
            severity: 'critical',
            retest: { kind: 'record', id: 'rec-9' },
        });
        expect(formToRecord(filledForm(), 'alice')).toMatchObject({
            findings_severity: null,
            tester: null,
        });
    });

    it('sends the TLPT block for a record of type tlpt only', () => {
        const form = {
            ...filledForm(),
            exercise_type: 'tlpt',
            tlpt_attestation_ref: 'ATT-1',
            tlpt_attested: '2026-11-02',
            tlpt_summary_ref: '',
        };
        expect(formToRecord(form, 'alice').tlpt).toEqual({
            attestation_ref: 'ATT-1',
            attested_at_ns: local(2026, 10, 2),
            summary_report_ref: null,
        });
        expect(formToRecord({ ...form, exercise_type: 'pentest' }, 'alice').tlpt).toBeNull();
        expect(formToRecord({ ...filledForm(), exercise_type: 'tlpt' }, 'alice').tlpt).toBeNull();
    });

    it('keeps the links of a draft', () => {
        const links = [{ kind: 'function', linked_id: 'payments', label: null }];
        expect(formToRecord({ ...filledForm(), links }, 'alice').links).toEqual(links);
    });
});

describe('a draft read back into the form', () => {
    const experiment = {
        id: 'rec-1',
        experiment_name: 'payments-db failover',
        exercise_type: 'failover',
        executed_at_ns: local(2026, 10, 20, 10, 0),
        hypothesis: 'h',
        method: 'm',
        outcome_status: 'passed',
        hypothesis_met: true,
        findings: null,
        action_items: ['Update the runbook'],
        target_system: 'payments-db',
        target_environment: 'production',
        blast_radius: null,
        recovery_time_s: 0,
        duration_s: null,
        renewal_due_ns: null,
        framework_refs: null,
        attestation: 'I attest',
        status: 'draft',
        rto_target_s: 900,
        rpo_target_s: null,
        rpo_measured_s: 4,
        data_verified: true,
        participants: [{ name: 'bob', role: null }],
        plan_id: 'plan-1',
    } as unknown as ManualExperiment;

    it('holds the steps, the actions, the participants and the plan', () => {
        const detail: ManualDetail = {
            experiment,
            audit: [],
            attachments: [],
            steps: [
                {
                    ordinal: 1,
                    title: 'Stop the primary',
                    actor: null,
                    started_at_ns: local(2026, 10, 20, 10, 0, 5),
                    ended_at_ns: null,
                    result: 'done',
                    note: null,
                },
            ],
            actions: [
                {
                    ordinal: 1,
                    title: 'Raise the pool size',
                    owner: 'dave',
                    due_at_ns: local(2026, 11, 1, 23, 59, 59, 999),
                    ticket: null,
                },
            ],
            links: [{ ordinal: 1, kind: 'function', linked_id: 'payments', label: null }],
        };
        const form = detailToForm(detail);
        expect(form.steps).toEqual([
            {
                title: 'Stop the primary',
                actor: '',
                started: '2026-11-20T10:00:05',
                ended: '',
                result: 'done',
                note: '',
            },
        ]);
        expect(form.participants).toEqual([{ name: 'bob', role: '' }]);
        expect(form.links).toEqual([{ kind: 'function', linked_id: 'payments', label: null }]);
        expect(form).toMatchObject({
            plan_id: 'plan-1',
            rto_target_s: '900',
            rpo_target_s: '',
            rpo_measured_s: '4',
            recovery_time_s: '0',
            data_verified: 'yes',
            executed: '2026-11-20T10:00',
        });
        // The older plain action becomes an action that still needs an owner.
        expect(form.actions).toEqual([
            { ...emptyAction(), title: 'Raise the pool size', owner: 'dave', due: '2026-12-01' },
            { ...emptyAction(), title: 'Update the runbook' },
        ]);
    });

    it('holds the severities, the tester, the TLPT block and the retests', () => {
        const tlpt = {
            ...experiment,
            exercise_type: 'tlpt',
            findings_severity: 'high',
            tester: 'Red Team Ltd',
            tester_kind: 'external',
            tlpt_attestation_ref: 'ATT-1',
            tlpt_attested_at_ns: local(2026, 10, 2, 12, 0),
            tlpt_summary_ref: 'TSR-1',
        } as ManualExperiment;
        const form = detailToForm({
            experiment: { ...tlpt, action_items: [] },
            audit: [],
            attachments: [],
            actions: [
                {
                    title: 'Remove stale accounts',
                    owner: 'carol',
                    severity: 'critical',
                    retest_kind: 'run',
                    retest_id: 'exp-1',
                },
            ],
        });
        expect(form).toMatchObject({
            findings_severity: 'high',
            tester_name: 'Red Team Ltd',
            tester_kind: 'external',
            tlpt_attestation_ref: 'ATT-1',
            tlpt_attested: '2026-11-02',
            tlpt_summary_ref: 'TSR-1',
        });
        expect(form.actions).toEqual([
            {
                ...emptyAction(),
                title: 'Remove stale accounts',
                owner: 'carol',
                severity: 'critical',
                retest_kind: 'run',
                retest_id: 'exp-1',
            },
        ]);
    });

    it('reads a record stored before the steps existed', () => {
        const form = detailToForm({ experiment, audit: [], attachments: [] });
        expect(form.steps).toEqual([]);
        expect(form.links).toEqual([]);
    });
});

describe('the plans a record can fulfil', () => {
    const plans = [
        plan({ id: 'same' }),
        plan({ id: 'no-target', target_system: null, target_environment: null }),
        plan({ id: 'other-target', target_system: 'ledger' }),
        plan({ id: 'other-type', exercise_type: 'restore' }),
        plan({ id: 'done', status: 'fulfilled' }),
        plan({ id: 'staging', target_environment: 'staging' }),
    ];

    it('are the planned plans of the same type and target', () => {
        const form = { ...filledForm() };
        expect(matchingPlans(plans, form).map((p) => p.id)).toEqual(['same', 'no-target']);
    });

    it('are every planned plan of the type while no target is typed', () => {
        const form = { ...filledForm(), target_system: '', target_environment: '' };
        expect(matchingPlans(plans, form).map((p) => p.id)).toEqual([
            'same',
            'no-target',
            'other-target',
            'staging',
        ]);
    });

    it('fill what the form leaves blank and keep what it holds', () => {
        const form = {
            ...emptyForm(),
            exercise_type: 'failover',
            target_environment: 'dr-site',
            rpo_target_s: 30,
        };
        const picked = applyPlan(form, plan({ function_id: 'payments' }));
        expect(picked).toMatchObject({
            plan_id: 'plan-1',
            target_system: 'payments-db',
            target_environment: 'dr-site',
            rto_target_s: 900,
            rpo_target_s: 30,
        });
        expect(picked.links).toEqual([{ kind: 'function', linked_id: 'payments', label: null }]);
        // Picked again, the function link is not doubled.
        expect(applyPlan(picked, plan({ function_id: 'payments' })).links).toHaveLength(1);
    });
});

describe('an overdue plan', () => {
    const now = local(2026, 11, 2);

    it('is a planned plan past its due date', () => {
        expect(isOverdue(plan(), now)).toBe(true);
        expect(isOverdue(plan({ due_at_ns: local(2026, 11, 3) }), now)).toBe(false);
    });

    it('is never a fulfilled or a cancelled plan', () => {
        expect(isOverdue(plan({ status: 'fulfilled' }), now)).toBe(false);
        expect(isOverdue(plan({ status: 'cancelled' }), now)).toBe(false);
    });
});

describe('the plan form', () => {
    it('sends the due date as the end of that day, and leaves out a blank due date', () => {
        const form = {
            ...emptyPlanForm(),
            name: 'restore test',
            exercise_type: 'restore',
            owner: 'alice',
            due: '2026-12-01',
            planned: '2026-11-30',
        };
        expect(planRequest(form, 'alice')).toMatchObject({
            name: 'restore test',
            exercise_type: 'restore',
            due_at_ns: dayEndNs('2026-12-01'),
            planned_at_ns: local(2026, 10, 30),
            interval_days: null,
            target_system: null,
            created_by: 'alice',
        });
        expect(planRequest({ ...form, due: '' }, 'alice')).not.toHaveProperty('due_at_ns');
    });

    it('reads a plan back and sends every field on an edit, a blank as a clear', () => {
        const form = planToForm(plan());
        expect(form).toMatchObject({
            due: '2026-12-01',
            interval_days: '365',
            target_system: 'payments-db',
        });
        const patch = planPatch({ ...form, interval_days: '', scope: '' }, 'bob');
        expect(patch).toMatchObject({
            interval_days: null,
            scope: null,
            due_at_ns: dayEndNs('2026-12-01'),
            updated_by: 'bob',
        });
        expect(patch).not.toHaveProperty('created_by');
    });
});

describe('the import body', () => {
    it('wraps a list of records and names who enters each one', () => {
        const parsed = importBody('[{"experiment_name": "a"}, {"entered_by": "eve"}]', 'q3', 'al');
        expect(parsed).toEqual({
            body: {
                label: 'q3',
                records: [{ experiment_name: 'a', entered_by: 'al' }, { entered_by: 'eve' }],
            },
        });
    });

    it('takes the body the API takes as it is', () => {
        const text = '{"label": "batch", "records": [{"x": 1}]}';
        expect(importBody(text, '', '')).toEqual({
            body: { label: 'batch', records: [{ x: 1 }] },
        });
    });

    it('says when the text is not JSON', () => {
        expect(importBody('{not json', '', 'al')).toMatchObject({
            error: expect.stringMatching(/^The import is not JSON: /),
        });
    });
});

describe('the words of the page', () => {
    it('give a size in bytes, KB or MB', () => {
        expect(fmtBytes(512)).toBe('512 B');
        expect(fmtBytes(1536)).toBe('1.5 KB');
        expect(fmtBytes(26_214_400)).toBe('25.0 MB');
        expect(fmtBytes(null)).toBe('—');
    });

    it('say how many sign-offs are still needed', () => {
        expect(signoffSummary('submitted', 1, 2, 1)).toBe(
            '1 of 2 approvals. 1 more sign-off needed.',
        );
        expect(signoffSummary('submitted', 0, 2, 2)).toBe(
            '0 of 2 approvals. 2 more sign-offs needed.',
        );
        expect(signoffSummary('verified', 2, 2, 0)).toBe('Verified with 2 of 2 approvals.');
        expect(signoffSummary('rejected', 1, 2, 0)).toBe('Rejected.');
    });

    it('judge a recovery against its target', () => {
        expect(recoveryVerdict(900, 600)).toBe('met');
        expect(recoveryVerdict(900, 900)).toBe('met');
        expect(recoveryVerdict(60, 61)).toBe('missed');
        expect(recoveryVerdict(null, 61)).toBeNull();
        expect(recoveryVerdict(60, null)).toBeNull();
    });

    it('write a time for a datetime field to the second', () => {
        expect(localInput(local(2026, 10, 20, 10, 0, 5), true)).toBe('2026-11-20T10:00:05');
        expect(localInput(null)).toBe('');
    });
});

describe('the target of the page', () => {
    const rows = [
        { id: 'a', target_system: 'payments-db', target_environment: 'production' },
        { id: 'b', target_system: 'payments-db', target_environment: 'staging' },
        { id: 'c', target_system: 'ledger', target_environment: 'production' },
        { id: 'd', target_system: null, target_environment: null },
    ];
    const ids = (filter: { target: string; environment: string }) =>
        ofTarget(rows, filter).map((r) => r.id);

    it('is read from the target and the environment of the URL, as the DR page reads them', () => {
        const query = new URLSearchParams('tab=records&target=payments-db&environment=production');
        expect(targetFilterOf(query)).toEqual({
            target: 'payments-db',
            environment: 'production',
        });
        expect(targetFilterOf(new URLSearchParams('tab=plans'))).toEqual({
            target: '',
            environment: '',
        });
    });

    it('keeps the rows of the target in the environment, and every row when it names none', () => {
        expect(ids({ target: 'payments-db', environment: 'production' })).toEqual(['a']);
        expect(ids({ target: 'payments-db', environment: '' })).toEqual(['a', 'b']);
        expect(ids({ target: '', environment: 'production' })).toEqual(['a', 'c']);
        expect(ids({ target: '', environment: '' })).toEqual(['a', 'b', 'c', 'd']);
        expect(ids({ target: 'search', environment: '' })).toEqual([]);
    });

    it('is the query a link to the Manual page or the DR page sends', () => {
        expect(targetQuery('payments db', 'production')).toBe(
            'target=payments+db&environment=production',
        );
        expect(targetQuery('payments-db', null)).toBe('target=payments-db');
        const back = targetFilterOf(new URLSearchParams(targetQuery('a&b=c', 'x y')));
        expect(back).toEqual({ target: 'a&b=c', environment: 'x y' });
    });
});
