import { describe, expect, it } from 'vitest';

import { historyStateText, proofDaysText, snapshotSkippedText } from './dora';
import type { MaturityHistoryPoint } from './types';

const point = (over: Partial<MaturityHistoryPoint>): MaturityHistoryPoint => ({
    quarter: '2026-Q2',
    state: 'stored',
    source: 'snapshot',
    as_of: '2026-06-30',
    as_of_ns: 1,
    level: 2,
    index: 2.4,
    unknown_share: 0.5,
    reason: null,
    ...over,
});

describe('historyStateText', () => {
    it('names the day a quarter was stored on', () => {
        expect(historyStateText(point({}))).toBe('stored 2026-06-30');
        expect(historyStateText(point({ source: null }))).toBe('stored 2026-06-30');
    });

    it('says when the proof tool stored the day', () => {
        expect(historyStateText(point({ source: 'proof' }))).toBe(
            'stored 2026-06-30 by the proof tool',
        );
    });

    it('does not put a source on today or on a gap', () => {
        expect(historyStateText(point({ state: 'today', source: 'proof' }))).toBe('today');
        expect(historyStateText(point({ state: 'no_data', source: 'proof', as_of: null }))).toBe(
            'no data',
        );
    });
});

describe('snapshotSkippedText', () => {
    it('counts in the singular and the plural', () => {
        expect(snapshotSkippedText(1)).toBe('1 day skipped since the daemon started');
        expect(snapshotSkippedText(3)).toBe('3 days skipped since the daemon started');
    });

    it('says nothing before the first skipped day', () => {
        expect(snapshotSkippedText(0)).toBe('');
    });
});

describe('proofDaysText', () => {
    it('counts what counts when the setting is on', () => {
        expect(proofDaysText({ enabled: true, counted: 1, ignored: 0, note: 'n' })).toBe(
            '1 proof day counted',
        );
        expect(proofDaysText({ enabled: true, counted: 0, ignored: 0, note: 'n' })).toBe(
            '0 proof days counted',
        );
    });

    it('counts what is left out when the setting is off, and is silent with none', () => {
        expect(proofDaysText({ enabled: false, counted: 0, ignored: 4, note: null })).toBe(
            '4 days stored by the proof tool are not counted',
        );
        expect(proofDaysText({ enabled: false, counted: 0, ignored: 1, note: null })).toBe(
            '1 day stored by the proof tool is not counted',
        );
        expect(proofDaysText({ enabled: false, counted: 0, ignored: 0, note: null })).toBe('');
    });
});
