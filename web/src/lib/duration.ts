import type { ExperimentRow } from './types';

/**
 * How long an experiment ran, in nanoseconds: the journal's own measure
 * when the run has one. The root span is longer, because it also covers
 * the wait that seals the run after the last step.
 */
export function experimentDurationNs(
    row: Pick<ExperimentRow, 'duration_ms' | 'duration_ns'>,
): number | null {
    const text = row.duration_ms?.trim() ?? '';
    const ms = text === '' ? NaN : Number(text);
    if (Number.isFinite(ms)) return ms * 1e6;
    return row.duration_ns;
}
