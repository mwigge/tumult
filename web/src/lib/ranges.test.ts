import { describe, expect, it } from 'vitest';

import { bucketAxis, rangeAxis } from './ranges';

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

    it('GIVEN a bucket older than the range WHEN the axis is built THEN that bar is shown whole', () => {
        const old = Date.UTC(2026, 10, 14, 11);
        expect(bucketAxis('24h', HOUR_S, NOW, [old, old + HOUR_S * 1000]).min).toBe(
            old - (HOUR_S * 1000) / 2,
        );
    });

    it('GIVEN a bucket that starts after now WHEN the axis is built THEN that bar is shown whole', () => {
        const ahead = Date.UTC(2026, 10, 16, 8);
        expect(bucketAxis('24h', HOUR_S, NOW, [ahead - HOUR_S * 1000, ahead]).max).toBe(
            ahead + (HOUR_S * 1000) / 2,
        );
    });

    it('GIVEN buckets inside the range WHEN the axis is built THEN they do not move it', () => {
        const inside = [Date.UTC(2026, 10, 16, 5), Date.UTC(2026, 10, 16, 7)];
        expect(bucketAxis('24h', HOUR_S, NOW, inside)).toEqual(bucketAxis('24h', HOUR_S, NOW));
    });

    it('GIVEN an unknown range WHEN the axis is built THEN it covers the last 24 hours', () => {
        expect(bucketAxis('bogus', HOUR_S, NOW)).toEqual(bucketAxis('24h', HOUR_S, NOW));
    });
});

describe('rangeAxis', () => {
    it('GIVEN a range WHEN the axis is built THEN it covers that range and ends at now', () => {
        expect(rangeAxis('7d', NOW)).toEqual({ min: NOW - 7 * DAY_S * 1000, max: NOW });
    });

    it('GIVEN points inside the range WHEN the axis is built THEN they do not move it', () => {
        expect(rangeAxis('24h', NOW, [NOW - HOUR_S * 1000])).toEqual(rangeAxis('24h', NOW));
    });

    it('GIVEN a point older than the range WHEN the axis is built THEN it starts at that point', () => {
        const old = NOW - 3 * DAY_S * 1000;
        expect(rangeAxis('24h', NOW, [old, NOW - 1]).min).toBe(old);
    });

    it('GIVEN a point after now WHEN the axis is built THEN it ends at that point', () => {
        const ahead = NOW + HOUR_S * 1000;
        expect(rangeAxis('24h', NOW, [ahead]).max).toBe(ahead);
    });

    it('GIVEN an unknown range WHEN the axis is built THEN it covers the last 24 hours', () => {
        expect(rangeAxis('bogus', NOW)).toEqual(rangeAxis('24h', NOW));
    });
});
