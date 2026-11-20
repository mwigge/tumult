// The tone of the end of a run: one table for the badges and the chart bands.

/** How a status reads: good, in between, bad, or no judgement. */
export type Tone = 'ok' | 'warn' | 'fail' | 'neutral';

/**
 * The tone of each status a run can end with, in the spelling of the
 * journal. It follows the verdict of the lake (`tumult_lake::run_verdict`):
 * a run that passed is ok, a run that failed is fail, and a run that proves
 * nothing either way is neutral. `tumult-lake/tests/run-verdicts.json` lists
 * the same statuses, and `tone.test.ts` fails when the two differ.
 */
export const RUN_TONE: Readonly<Record<string, Tone>> = {
    completed: 'ok',
    deviated: 'fail',
    failed: 'fail',
    halted: 'fail',
    aborted: 'neutral',
    interrupted: 'neutral',
};

/** The tone of `status` when it is a status a run ends with, in any case. */
export function runTone(status: string | null): Tone | undefined {
    const key = (status ?? '').toLowerCase();
    return Object.hasOwn(RUN_TONE, key) ? RUN_TONE[key] : undefined;
}
