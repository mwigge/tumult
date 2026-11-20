import type { ExperimentRow, RunRow } from './types';

/** The three instants of a run the times below are read from. */
type RunInstants = Pick<RunRow, 'queued_at_ns' | 'started_at_ns' | 'ended_at_ns'>;

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

/**
 * How long a run ran, in nanoseconds: from its start to its end, or to
 * `nowNs` while it is still going. A run that never started has none.
 */
export function runTimeNs(run: RunInstants, nowNs: number): number | null {
    if (run.started_at_ns === null) return null;
    return (run.ended_at_ns ?? nowNs) - run.started_at_ns;
}

/**
 * How long a run waited before it started, in nanoseconds. A run that has
 * not started waits until it ends, or until `nowNs`.
 */
export function runWaitNs(run: RunInstants, nowNs: number): number {
    return (run.started_at_ns ?? run.ended_at_ns ?? nowNs) - run.queued_at_ns;
}
