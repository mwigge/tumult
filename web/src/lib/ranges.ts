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
