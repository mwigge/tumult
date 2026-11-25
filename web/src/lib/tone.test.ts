import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

import { chartTokens } from './echarts';
import { outcomeColor } from './overlays';
import { RUN_TONE, TONES, toneOf } from './tone';
import type { Tone, ToneKind } from './tone';

type Verdict = 'pass' | 'fail' | 'no_evidence';

/**
 * The statuses a run can end with and the verdict of each, as the lake
 * decides it. A test of tumult-lake checks the file against its own table.
 */
const VERDICTS: Record<string, Verdict> = JSON.parse(
    readFileSync(
        fileURLToPath(new URL('../../../tumult-lake/tests/run-verdicts.json', import.meta.url)),
        'utf8',
    ),
);

/** A run that passed is ok, one that failed is fail, one that proves nothing is neutral. */
const BADGE: Record<Verdict, string> = { pass: 'ok', fail: 'fail', no_evidence: 'neutral' };
const T = chartTokens('tumult-dark');
const BAND: Record<Verdict, string> = { pass: T.ok, fail: T.fail, no_evidence: T.text };

describe('the tone of the end of a run', () => {
    it('has a tone for each status the lake has a verdict for, and for no other', () => {
        expect(Object.keys(RUN_TONE).sort()).toEqual(Object.keys(VERDICTS).sort());
    });

    it.each(Object.entries(VERDICTS))(
        'draws the badge of the status %s as its verdict %s',
        (status, verdict) => {
            expect(toneOf('status', status)).toBe(BADGE[verdict]);
            expect(toneOf('status', status.toUpperCase())).toBe(BADGE[verdict]);
        },
    );

    it.each(Object.entries(VERDICTS))(
        'draws the band of the status %s as its verdict %s',
        (status, verdict) => {
            expect(outcomeColor(status, T)).toBe(BAND[verdict]);
            expect(outcomeColor(status.toUpperCase(), T)).toBe(BAND[verdict]);
        },
    );
});

describe('the tone of what is not the end of a run', () => {
    it('draws a manual outcome between a pass and a failure as warn', () => {
        expect(toneOf('status', 'partial')).toBe('warn');
    });

    it('keeps the tone of a run state that is not an ending', () => {
        expect(toneOf('status', 'passed')).toBe('ok');
        expect(toneOf('status', 'rollback_pending')).toBe('warn');
        expect(toneOf('status', 'orphaned')).toBe('fail');
        expect(toneOf('status', 'running')).toBe('neutral');
        expect(toneOf('status', null)).toBe('neutral');
        expect(outcomeColor(null, T)).toBe(T.text);
    });

    it('reads a word every object has as no ending of a run', () => {
        expect(toneOf('status', 'constructor')).toBe('neutral');
        expect(outcomeColor('toString', T)).toBe(T.text);
    });
});

/** The four steps OTel gives a severity: `warn`, then `warn2` to `warn4`. */
const steps = (name: string, tone: Tone): [string, Tone][] =>
    ['', '2', '3', '4'].map((step) => [`${name}${step}`, tone]);

/** The run statuses, which the events of a run share. */
const STATUS: [string, Tone][] = [
    ['completed', 'ok'],
    ['deviated', 'fail'],
    ['failed', 'fail'],
    ['halted', 'fail'],
    ['aborted', 'neutral'],
    ['interrupted', 'neutral'],
    ['success', 'ok'],
    ['passed', 'ok'],
    ['partial', 'warn'],
    ['rollback_pending', 'warn'],
    ['rejected', 'warn'],
    ['expired', 'warn'],
    ['failure', 'fail'],
    ['orphaned', 'fail'],
];

/** Every word of every kind and the tone it is shown in. */
const EXPECTED: Record<ToneKind, [string, Tone][]> = {
    status: STATUS,
    event: [...STATUS, ['approved', 'ok'], ['requested', 'warn'], ['stop_requested', 'warn']],
    tier: [
        ['T1', 'neutral'],
        ['T2', 'warn'],
        ['T3', 'fail'],
    ],
    severity: [
        ...steps('trace', 'neutral'),
        ...steps('debug', 'neutral'),
        ...steps('info', 'ok'),
        ...steps('warn', 'warn'),
        ['warning', 'warn'],
        ...steps('error', 'fail'),
        ['err', 'fail'],
        ...steps('fatal', 'fail'),
    ],
    finding: [
        ['low', 'neutral'],
        ['medium', 'warn'],
        ['high', 'fail'],
        ['critical', 'fail'],
    ],
    verdict: [
        ['met', 'ok'],
        ['pass', 'ok'],
        ['passed', 'ok'],
        ['improved', 'ok'],
        ['inconclusive', 'warn'],
        ['warn', 'warn'],
        ['breached', 'fail'],
        ['not met', 'fail'],
        ['missed', 'fail'],
        ['not_recovered', 'fail'],
        ['regressed', 'fail'],
        ['fail', 'fail'],
        ['blocked', 'fail'],
        ['not_measured', 'neutral'],
        ['not checked', 'neutral'],
        ['unchanged', 'neutral'],
    ],
    dr: [
        ['ok', 'ok'],
        ['due', 'warn'],
        ['overdue', 'fail'],
        ['never_tested', 'neutral'],
    ],
    scenario: [
        ['passed', 'ok'],
        ['overdue', 'warn'],
        ['failed', 'fail'],
        ['missing', 'neutral'],
    ],
    standing: [
        ['meets', 'ok'],
        ['gap', 'fail'],
        ['unknown', 'neutral'],
        ['no_events', 'neutral'],
        ['na', 'neutral'],
    ],
    register: [
        ['ok', 'ok'],
        ['stale', 'warn'],
        ['invalid', 'fail'],
        ['none', 'neutral'],
    ],
    review: [
        ['verified', 'ok'],
        ['submitted', 'warn'],
        ['rejected', 'fail'],
        ['draft', 'neutral'],
    ],
    plan: [
        ['fulfilled', 'ok'],
        ['planned', 'warn'],
        ['cancelled', 'neutral'],
    ],
    decision: [
        ['approved', 'ok'],
        ['rejected', 'fail'],
    ],
    step: [
        ['succeeded', 'ok'],
        ['done', 'ok'],
        ['failed', 'fail'],
        ['timeout', 'fail'],
    ],
    rollback: [
        ['completed', 'ok'],
        ['failed', 'fail'],
    ],
    cover: [
        ['covered', 'ok'],
        ['stale', 'warn'],
        ['missing', 'neutral'],
    ],
    level: [
        ['spine', 'ok'],
        ['target', 'warn'],
        ['none', 'fail'],
    ],
    band: [
        ['good', 'ok'],
        ['fair', 'warn'],
        ['poor', 'fail'],
    ],
    token: [
        ['active', 'ok'],
        ['expired', 'warn'],
        ['revoked', 'fail'],
    ],
    catalog: [
        ['action', 'warn'],
        ['probe', 'neutral'],
    ],
    switch: [
        ['enabled', 'ok'],
        ['connected', 'ok'],
        ['disabled', 'neutral'],
        ['not connected', 'neutral'],
    ],
    report: [
        ['manual', 'warn'],
        ['scheduled', 'neutral'],
    ],
};

const KINDS = Object.keys(EXPECTED) as ToneKind[];
const ROWS = KINDS.flatMap((kind) =>
    EXPECTED[kind].map(([value, tone]) => [kind, value, tone] as const),
);

describe('the tone of each kind of word a badge shows', () => {
    it.each(ROWS)('shows the %s %s as %s', (kind, value, tone) => {
        expect(toneOf(kind, value)).toBe(tone);
    });

    it('has the kinds of this test, and a row here for each word of each kind', () => {
        expect(Object.keys(TONES).sort()).toEqual([...KINDS].sort());
        for (const kind of KINDS) {
            const words = EXPECTED[kind].map(([value]) => value).sort();
            expect(Object.keys(TONES[kind]).sort(), kind).toEqual(words);
        }
    });

    it.each(KINDS)('shows a %s it does not know, or none, as neutral', (kind) => {
        expect(toneOf(kind, 'no such word')).toBe('neutral');
        expect(toneOf(kind, '')).toBe('neutral');
        expect(toneOf(kind, null)).toBe('neutral');
        expect(toneOf(kind, undefined)).toBe('neutral');
        expect(toneOf(kind, 'constructor')).toBe('neutral');
        expect(toneOf(kind, '__proto__')).toBe('neutral');
    });

    it('reads a word in any case', () => {
        expect(toneOf('severity', 'ERROR')).toBe('fail');
        expect(toneOf('severity', 'Warning')).toBe('warn');
        expect(toneOf('tier', 't3')).toBe('fail');
        expect(toneOf('dr', 'OVERDUE')).toBe('fail');
    });

    it('shows a finding low as neutral, medium as warn, high and critical as fail', () => {
        expect(['low', 'medium', 'high', 'critical'].map((s) => toneOf('finding', s))).toEqual([
            'neutral',
            'warn',
            'fail',
            'fail',
        ]);
    });
});
