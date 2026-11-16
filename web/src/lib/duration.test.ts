import { describe, expect, it } from 'vitest';

import { experimentDurationNs, runTimeNs, runWaitNs } from './duration';

describe('experimentDurationNs', () => {
    it('GIVEN a journal duration WHEN the root span is longer THEN the journal wins', () => {
        expect(experimentDurationNs({ duration_ms: '1200', duration_ns: 3.6e9 })).toBe(1.2e9);
    });

    it('GIVEN no journal duration WHEN read THEN the span duration is used', () => {
        expect(experimentDurationNs({ duration_ms: null, duration_ns: 3.6e9 })).toBe(3.6e9);
    });

    it('GIVEN a journal duration that is not a number WHEN read THEN the span duration is used', () => {
        expect(experimentDurationNs({ duration_ms: 'n/a', duration_ns: 5e8 })).toBe(5e8);
        expect(experimentDurationNs({ duration_ms: ' ', duration_ns: 5e8 })).toBe(5e8);
    });

    it('GIVEN no duration at all WHEN read THEN there is none', () => {
        expect(experimentDurationNs({ duration_ms: null, duration_ns: null })).toBeNull();
    });
});

const QUEUED = 1_000e9;
const STARTED = QUEUED + 90e9;
const ENDED = STARTED + 12.5e9;
const NOW = ENDED + 60e9;

describe('runTimeNs', () => {
    it('GIVEN an ended run WHEN read THEN it is ended minus started', () => {
        const run = { queued_at_ns: QUEUED, started_at_ns: STARTED, ended_at_ns: ENDED };
        expect(runTimeNs(run, NOW)).toBe(12.5e9);
    });

    it('GIVEN a run that is still going WHEN read THEN it is the time since it started', () => {
        const run = { queued_at_ns: QUEUED, started_at_ns: STARTED, ended_at_ns: null };
        expect(runTimeNs(run, NOW)).toBe(72.5e9);
    });

    it('GIVEN a run that never started WHEN read THEN it has no run time', () => {
        expect(
            runTimeNs({ queued_at_ns: QUEUED, started_at_ns: null, ended_at_ns: null }, NOW),
        ).toBeNull();
        expect(
            runTimeNs({ queued_at_ns: QUEUED, started_at_ns: null, ended_at_ns: ENDED }, NOW),
        ).toBeNull();
    });
});

describe('runWaitNs', () => {
    it('GIVEN a started run WHEN read THEN it is started minus queued', () => {
        const run = { queued_at_ns: QUEUED, started_at_ns: STARTED, ended_at_ns: ENDED };
        expect(runWaitNs(run, NOW)).toBe(90e9);
    });

    it('GIVEN a run that still waits WHEN read THEN it is the time since it was queued', () => {
        const run = { queued_at_ns: QUEUED, started_at_ns: null, ended_at_ns: null };
        expect(runWaitNs(run, NOW)).toBe(162.5e9);
    });

    it('GIVEN a run that ended before it started WHEN read THEN the wait ends there', () => {
        const run = { queued_at_ns: QUEUED, started_at_ns: null, ended_at_ns: STARTED };
        expect(runWaitNs(run, NOW)).toBe(90e9);
    });
});
