import { describe, expect, it } from 'vitest';

import { bucketAxis } from './ranges';

const HOUR_S = 3600;
const DAY_S = 86_400;
// 2026-11-16 07:20 UTC: early in the day, so a daily bar centred on
// midnight would otherwise push the axis to noon, past now.
const NOW = Date.UTC(2026, 10, 16, 7, 20);

describe('bucketAxis', () => {
    it('GIVEN daily buckets WHEN the axis is built THEN it ends at now', () => {
        expect(bucketAxis('7d', DAY_S, NOW).max).toBe(NOW);
    });

    it('GIVEN daily buckets WHEN the axis is built THEN the first bucket is shown whole', () => {
        const firstBucket = Date.UTC(2026, 10, 9);
        expect(bucketAxis('7d', DAY_S, NOW).min).toBe(firstBucket - (DAY_S * 1000) / 2);
    });

    it('GIVEN hourly buckets WHEN the axis is built THEN it starts half a bucket before the first hour', () => {
        const firstBucket = Date.UTC(2026, 10, 15, 7);
        expect(bucketAxis('24h', HOUR_S, NOW)).toEqual({
            min: firstBucket - (HOUR_S * 1000) / 2,
            max: NOW,
        });
    });

    it('GIVEN an unknown range WHEN the axis is built THEN it covers the last 24 hours', () => {
        expect(bucketAxis('bogus', HOUR_S, NOW)).toEqual(bucketAxis('24h', HOUR_S, NOW));
    });
});
