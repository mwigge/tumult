import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

import { statusClass } from './api';
import { CHART } from './echarts';
import { outcomeColor } from './overlays';
import { RUN_TONE } from './tone';

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
const BAND: Record<Verdict, string> = { pass: CHART.ok, fail: CHART.fail, no_evidence: CHART.text };

describe('the tone of the end of a run', () => {
    it('has a tone for each status the lake has a verdict for, and for no other', () => {
        expect(Object.keys(RUN_TONE).sort()).toEqual(Object.keys(VERDICTS).sort());
    });

    it.each(Object.entries(VERDICTS))(
        'draws the badge of the status %s as its verdict %s',
        (status, verdict) => {
            expect(statusClass(status)).toBe(BADGE[verdict]);
            expect(statusClass(status.toUpperCase())).toBe(BADGE[verdict]);
        },
    );

    it.each(Object.entries(VERDICTS))(
        'draws the band of the status %s as its verdict %s',
        (status, verdict) => {
            expect(outcomeColor(status)).toBe(BAND[verdict]);
            expect(outcomeColor(status.toUpperCase())).toBe(BAND[verdict]);
        },
    );
});

describe('the tone of what is not the end of a run', () => {
    it('draws a manual outcome between a pass and a failure as warn', () => {
        expect(statusClass('partial')).toBe('warn');
    });

    it('keeps the tone of a run state that is not an ending', () => {
        expect(statusClass('passed')).toBe('ok');
        expect(statusClass('rollback_pending')).toBe('warn');
        expect(statusClass('orphaned')).toBe('fail');
        expect(statusClass('running')).toBe('neutral');
        expect(statusClass(null)).toBe('neutral');
        expect(outcomeColor(null)).toBe(CHART.text);
    });

    it('reads a word every object has as no ending of a run', () => {
        expect(statusClass('constructor')).toBe('neutral');
        expect(outcomeColor('toString')).toBe(CHART.text);
    });
});
