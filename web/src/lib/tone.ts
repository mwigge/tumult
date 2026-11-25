// The tone of each word a badge shows: one table for each kind of word,
// and the end of a run also for the chart bands.

import type {
    ApprovalTier,
    DrState,
    DrVerdict,
    ManualExperiment,
    ManualPlan,
    ManualSignOffRow,
    PerfVerdictKind,
    RegisterState,
    RiskSeverity,
    ScenarioState,
    Standing,
} from './types';

/** How a status reads: good, in between, bad, or no judgement. */
export type Tone = 'ok' | 'warn' | 'fail' | 'neutral';

/**
 * The tone of each status a run can end with, in the spelling of the
 * journal. It follows the verdict of the lake (`tumult_lake::run_verdict`):
 * a run that passed is ok, a run that failed is fail, and a run that proves
 * nothing either way is neutral. `tumult-lake/tests/run-verdicts.json` lists
 * the same statuses, and `tone.test.ts` fails when the two differ.
 */
export const RUN_TONE: Readonly<Record<string, Tone>> = {
    completed: 'ok',
    deviated: 'fail',
    failed: 'fail',
    halted: 'fail',
    aborted: 'neutral',
    interrupted: 'neutral',
};

/** The tone of `status` when it is a status a run ends with, in any case. */
export function runTone(status: string | null): Tone | undefined {
    const key = (status ?? '').toLowerCase();
    return Object.hasOwn(RUN_TONE, key) ? RUN_TONE[key] : undefined;
}

/**
 * The status of a run, an experiment or a manual outcome: the end of a run,
 * and the states on the way. A manual outcome that worked in part, a
 * pending rollback, and a request that was rejected or expired before the
 * run started are in between.
 */
const STATUS: Readonly<Record<string, Tone>> = {
    ...RUN_TONE,
    success: 'ok',
    passed: 'ok',
    partial: 'warn',
    rollback_pending: 'warn',
    rejected: 'warn',
    expired: 'warn',
    failure: 'fail',
    orphaned: 'fail',
};

/** The four steps OTel gives a severity: `warn`, then `warn2` to `warn4`. */
const steps = (name: string, tone: Tone): Record<string, Tone> =>
    Object.fromEntries(['', '2', '3', '4'].map((step) => [`${name}${step}`, tone]));

/** The words of a verdict that the API sends or a page writes. */
type Verdict =
    | DrVerdict
    | PerfVerdictKind
    | 'not met'
    | 'missed'
    | 'not checked'
    | 'pass'
    | 'warn'
    | 'fail'
    | 'passed'
    | 'blocked';

/** The tone of each word, by the kind of word. A word not in its table is neutral. */
export const TONES = {
    /** The status of a run, an experiment or a manual outcome. */
    status: STATUS,
    /** An event of the audit trail of a run: the statuses, and the approval and stop events. */
    event: { ...STATUS, approved: 'ok', requested: 'warn', stop_requested: 'warn' },
    /** The approval tier of a run. */
    tier: { T1: 'neutral', T2: 'warn', T3: 'fail' } satisfies Record<ApprovalTier, Tone>,
    /** The severity of a log record, in the OTel names. */
    severity: {
        ...steps('trace', 'neutral'),
        ...steps('debug', 'neutral'),
        ...steps('info', 'ok'),
        ...steps('warn', 'warn'),
        warning: 'warn',
        ...steps('error', 'fail'),
        err: 'fail',
        ...steps('fatal', 'fail'),
    },
    /** The severity of a finding: a risk, a manual exercise or one of its actions. */
    finding: {
        low: 'neutral',
        medium: 'warn',
        high: 'fail',
        critical: 'fail',
    } satisfies Record<RiskSeverity, Tone>,
    /** A verdict: of an objective, a steady state, a DR test, a gate or a comparison. */
    verdict: {
        met: 'ok',
        pass: 'ok',
        passed: 'ok',
        improved: 'ok',
        inconclusive: 'warn',
        warn: 'warn',
        breached: 'fail',
        'not met': 'fail',
        missed: 'fail',
        not_recovered: 'fail',
        regressed: 'fail',
        fail: 'fail',
        blocked: 'fail',
        not_measured: 'neutral',
        'not checked': 'neutral',
        unchanged: 'neutral',
    } satisfies Record<Verdict, Tone>,
    /** The DR state of a target: when its next test is due. */
    dr: {
        ok: 'ok',
        due: 'warn',
        overdue: 'fail',
        never_tested: 'neutral',
    } satisfies Record<DrState, Tone>,
    /** A plan scenario of RTS 2024/1774 for a function. */
    scenario: {
        passed: 'ok',
        overdue: 'warn',
        failed: 'fail',
        missing: 'neutral',
    } satisfies Record<ScenarioState, Tone>,
    /** How an entry of the DORA score stands. */
    standing: {
        meets: 'ok',
        gap: 'fail',
        unknown: 'neutral',
        no_events: 'neutral',
        na: 'neutral',
    } satisfies Record<Standing, Tone>,
    /** The state of the function register of the org file. */
    register: {
        ok: 'ok',
        stale: 'warn',
        invalid: 'fail',
        none: 'neutral',
    } satisfies Record<RegisterState, Tone>,
    /** The review of a manual record: submitted records wait for a sign-off. */
    review: {
        verified: 'ok',
        submitted: 'warn',
        rejected: 'fail',
        draft: 'neutral',
    } satisfies Record<ManualExperiment['status'], Tone>,
    /** The status of a plan of a manual exercise. */
    plan: {
        fulfilled: 'ok',
        planned: 'warn',
        cancelled: 'neutral',
    } satisfies Record<ManualPlan['status'], Tone>,
    /** The decision of a sign-off. */
    decision: {
        approved: 'ok',
        rejected: 'fail',
    } satisfies Record<ManualSignOffRow['decision'], Tone>,
    /** How a step of a run or of a manual exercise ended. */
    step: { succeeded: 'ok', done: 'ok', failed: 'fail', timeout: 'fail' },
    /** How the rollback of a run ended. */
    rollback: { completed: 'ok', failed: 'fail' },
    /** How well a discipline is covered by evidence. */
    cover: { covered: 'ok', stale: 'warn', missing: 'neutral' },
    /** How much of the resilience metadata a service sends: the ICT spine, the target or none. */
    level: { spine: 'ok', target: 'warn', none: 'fail' },
    /** The band of a posture score. */
    band: { good: 'ok', fair: 'warn', poor: 'fail' },
    /** The state of an API token. */
    token: { active: 'ok', expired: 'warn', revoked: 'fail' },
    /** What a step of the catalogue does: an action changes the target, a probe reads it. */
    catalog: { action: 'warn', probe: 'neutral' },
    /** Something that is on or off: a schedule, a webhook or a feed. */
    switch: { enabled: 'ok', connected: 'ok', disabled: 'neutral', 'not connected': 'neutral' },
    /** How a report file was made: by hand or by a schedule. */
    report: { manual: 'warn', scheduled: 'neutral' },
} as const satisfies Record<string, Readonly<Record<string, Tone>>>;

/** A kind of word that has a table of tones. */
export type ToneKind = keyof typeof TONES;

/** Each table with its words in lower case, so a word is read in any case. */
const LOWER = new Map(
    Object.entries(TONES).map(([kind, table]) => [
        kind,
        new Map(Object.entries(table).map(([word, tone]) => [word.toLowerCase(), tone as Tone])),
    ]),
);

/** The tone of `value`, a word of `kind`, in any case. A word the table has not is neutral. */
export function toneOf(kind: ToneKind, value: string | null | undefined): Tone {
    return LOWER.get(kind)?.get((value ?? '').toLowerCase()) ?? 'neutral';
}
