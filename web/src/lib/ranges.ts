// The time ranges every ranged endpoint accepts, in one place. The API
// keeps the same table in tumult-api/src/sql_util.rs.

const DAY_S = 86_400;

/** Seconds per range, shortest first. */
export const RANGE_SECONDS: Record<string, number> = {
    '24h': DAY_S,
    '7d': 7 * DAY_S,
    '14d': 14 * DAY_S,
    '30d': 30 * DAY_S,
};

/** Every range the API accepts. */
export const RANGES = Object.keys(RANGE_SECONDS);

/** What a page offers unless it asks for the long window too. */
export const SHORT_RANGES = RANGES.filter((range) => range !== '30d');

/**
 * Bounds of a time axis that shows the whole of `range` up to now. A
 * point in `xsMs` outside that window widens the axis, so none is lost.
 */
export function rangeAxis(
    range: string,
    nowMs: number,
    xsMs: number[] = [],
): { min: number; max: number } {
    const rangeMs = (RANGE_SECONDS[range] ?? DAY_S) * 1000;
    return {
        min: xsMs.reduce((a, b) => Math.min(a, b), nowMs - rangeMs),
        max: xsMs.reduce((a, b) => Math.max(a, b), nowMs),
    };
}

/**
 * Bounds of a time axis for bars bucketed over `range`. A bar is centred
 * on the start of its bucket: the axis starts half a bucket before the
 * first one, so that bar is whole, and ends at now. A bucket in
 * `bucketsMs` that lies outside that window (the clock of the browser
 * and of the server differ) widens the axis, so no bar is lost.
 */
export function bucketAxis(
    range: string,
    bucketS: number,
    nowMs: number,
    bucketsMs: number[] = [],
): { min: number; max: number } {
    const rangeMs = (RANGE_SECONDS[range] ?? DAY_S) * 1000;
    const bucketMs = bucketS * 1000;
    const firstBucket = Math.floor((nowMs - rangeMs) / bucketMs) * bucketMs;
    const first = Math.min(firstBucket, ...bucketsMs);
    const last = Math.max(nowMs, ...bucketsMs);
    return { min: first - bucketMs / 2, max: last > nowMs ? last + bucketMs / 2 : nowMs };
}
