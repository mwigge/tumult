// The answers of the Manual API for the style snapshot of the Manual tabs:
// two plans, three records and one record in full, with its steps,
// actions, links, sign-offs, evidence and audit trail.
import type {
    ManualDetail,
    ManualExperiment,
    ManualPlan,
    ManualPlanDetail,
    ManualVerdict,
} from '../src/lib/types';
import { HOUR_NS, NOW_NS } from './chart-theme-data';

const plan = (id: string, name: string, dueNs: number): ManualPlan => ({
    id,
    name,
    exercise_type: 'failover',
    target_system: 'payments-api',
    target_environment: 'prod',
    function_id: 'fn-pay',
    scope: 'eu',
    owner: 'alice',
    planned_at_ns: NOW_NS - 48 * HOUR_NS,
    due_at_ns: dueNs,
    interval_days: 90,
    rto_target_s: 600,
    rpo_target_s: 60,
    status: 'planned',
    created_by: 'alice',
    created_at_ns: NOW_NS - 72 * HOUR_NS,
    updated_by: null,
    updated_at_ns: null,
});

/** One plan past its due date and one due in ten days. */
export const plans = [
    plan('plan-1', 'payments failover', NOW_NS - 24 * HOUR_NS),
    plan('plan-2', 'ledger restore', NOW_NS + 240 * HOUR_NS),
];

const planAudit: ManualPlanDetail['audit'] = [
    { id: 'a1', action: 'create', changed_by: 'alice', changed_at_ns: NOW_NS - 72 * HOUR_NS },
    { id: 'a2', action: 'edit', changed_by: 'bob', changed_at_ns: NOW_NS - 70 * HOUR_NS },
];

const record = (
    id: string,
    status: ManualExperiment['status'],
    outcome: string,
): ManualExperiment => ({
    id,
    experiment_name: `exercise ${id}`,
    exercise_type: 'failover',
    executed_at_ns: NOW_NS - 30 * HOUR_NS,
    hypothesis: 'the replica takes over',
    method: 'promote the replica',
    outcome_status: outcome,
    hypothesis_met: true,
    findings: 'slow DNS',
    action_items: [],
    target_system: 'payments-api',
    target_environment: 'prod',
    blast_radius: 'one region',
    recovery_time_s: 420,
    duration_s: 3600,
    origin: 'manual',
    entered_by: 'alice',
    entered_at_ns: NOW_NS - 29 * HOUR_NS,
    attestation: 'I attest that this ran.',
    status,
    reviewed_by: status === 'verified' ? 'bob' : null,
    reviewed_at_ns: null,
    review_note: null,
    renewal_due_ns: NOW_NS + 2000 * HOUR_NS,
    framework_refs: ['dora.art24.6'],
    scenarios: ['S1'],
    batch_id: null,
    content_hash: 'abcdef0123456789abcdef0123456789',
    rto_target_s: 600,
    rpo_target_s: 60,
    rpo_measured_s: 30,
    data_verified: true,
    participants: [{ name: 'alice', role: 'lead' }],
    plan_id: 'plan-1',
    hash_version: 2,
    findings_severity: 'low',
    tester: 'alice',
    tester_kind: 'internal',
});

/** A submitted record, a draft and a verified one. */
export const records = [
    record('rec-1', 'submitted', 'passed'),
    record('rec-2', 'draft', 'partial'),
    record('rec-3', 'verified', 'failed'),
];

const detail: ManualDetail = {
    experiment: records[0],
    audit: [
        {
            id: 'r1',
            experiment_id: 'rec-1',
            changed_by: 'alice',
            changed_at_ns: NOW_NS - 29 * HOUR_NS,
            action: 'create',
            diff: null,
            prev_hash: null,
            new_hash: 'h1',
        },
        {
            id: 'r2',
            experiment_id: 'rec-1',
            changed_by: 'alice',
            changed_at_ns: NOW_NS - 28 * HOUR_NS,
            action: 'submit',
            diff: null,
            prev_hash: 'h1',
            new_hash: 'h2',
        },
    ],
    attachments: [
        {
            id: 'at-1',
            experiment_id: 'rec-1',
            kind: 'file',
            uri: 'report.pdf',
            label: 'report',
            file_hash: 'ff00',
            media_type: 'application/pdf',
            size_bytes: 2048,
            added_by: 'alice',
            added_at_ns: NOW_NS - 28 * HOUR_NS,
        },
        {
            id: 'at-2',
            experiment_id: 'rec-1',
            kind: 'link',
            uri: 'https://example.com/run',
            label: 'ticket',
            file_hash: null,
            media_type: null,
            size_bytes: null,
            added_by: 'bob',
            added_at_ns: NOW_NS - 27 * HOUR_NS,
        },
    ],
    steps: [
        {
            ordinal: 1,
            title: 'stop primary',
            actor: 'alice',
            started_at_ns: NOW_NS - 30 * HOUR_NS,
            ended_at_ns: NOW_NS - 29.9 * HOUR_NS,
            result: 'done',
            note: 'quick',
        },
        {
            ordinal: 2,
            title: 'promote',
            actor: null,
            started_at_ns: null,
            ended_at_ns: null,
            result: 'failed',
            note: null,
        },
    ],
    actions: [
        {
            id: 'ac-1',
            ordinal: 1,
            title: 'fix DNS TTL',
            owner: 'carol',
            due_at_ns: NOW_NS + 100 * HOUR_NS,
            ticket: 'OPS-1',
            status: 'open',
            severity: 'low',
            retest_kind: 'run',
            retest_id: 'run-1',
        },
    ],
    links: [{ ordinal: 1, kind: 'function', linked_id: 'fn-pay', label: 'payments' }],
    signoffs: [
        {
            id: 's1',
            role: 'approver',
            signed_by: 'bob',
            signed_at_ns: NOW_NS - 20 * HOUR_NS,
            decision: 'approved',
            note: 'fine',
            signed_hash: 'x',
        },
    ],
    signoffs_required: 2,
    signoffs_needed: 1,
};

/** The verify of the record finds a broken chain; the verify of the plan finds none. */
const broken: ManualVerdict = {
    experiment_id: 'rec-1',
    ok: false,
    chain_valid: false,
    hash_version: 2,
    legacy_rows: 1,
    problems: [{ table: 'audit', row: 'r2', field: 'diff', problem: 'hash differs' }],
};
const whole: ManualVerdict = {
    plan_id: 'plan-1',
    ok: true,
    chain_valid: true,
    hash_version: 2,
    legacy_rows: 0,
    problems: [],
};

/** The Manual API by path. */
export const manualApi = {
    '/api/manual/plans': { plans },
    '/api/manual/plans/plan-1': { plan: plans[0], audit: planAudit },
    '/api/manual/plans/plan-2': { plan: plans[1], audit: planAudit },
    '/api/manual/plans/plan-1/audit/verify': whole,
    '/api/manual/experiments': { records },
    '/api/manual/experiments/rec-1': detail,
    '/api/manual/experiments/rec-1/audit/verify': broken,
};
