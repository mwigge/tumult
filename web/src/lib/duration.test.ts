import { describe, expect, it } from 'vitest';

import { experimentDurationNs } from './duration';

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
