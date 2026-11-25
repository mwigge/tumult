<script lang="ts">
    // The DR tests of the period, automated and manual together, newest
    // first, one page of the API at a time. Each row links to the page of
    // its run, or to the manual records.
    import {
        localDay,
        rpoAlarm,
        rpoText,
        rtoAlarm,
        rtoText,
        testHref,
        verdictLabel,
    } from '#lib/dr.js';
    import type { DrTest } from '#lib/types.js';
    import Badge from './Badge.svelte';
    import DrObjectiveCell from './DrObjectiveCell.svelte';
    import State from './State.svelte';

    let {
        tests,
        total,
        page,
        pageSize,
        canPrev,
        canNext,
        busy = false,
        onprev,
        onnext,
    }: {
        tests: DrTest[];
        /** The tests of the period. */
        total: number;
        /** The page shown, the first is 1. */
        page: number;
        pageSize: number;
        canPrev: boolean;
        canNext: boolean;
        busy?: boolean;
        onprev?: () => void;
        onnext?: () => void;
    } = $props();

    const first = $derived((page - 1) * pageSize + 1);
    const last = $derived((page - 1) * pageSize + tests.length);
    const origin = (test: DrTest) =>
        test.origin === 'manual' ? `manual ${test.exercise_type ?? ''}`.trim() : 'automated';
    const verified = (value: boolean | null) => (value === null ? '—' : value ? 'yes' : 'no');
</script>

{#if tests.length === 0}
    <State kind="empty">No DR test in this period.</State>
{:else}
    <table class="data">
        <caption class="sr-only">The DR tests of the period, newest first</caption>
        <thead>
            <tr>
                <th scope="col">Executed</th>
                <th scope="col">Test</th>
                <th scope="col">Origin</th>
                <th scope="col">Target</th>
                <th scope="col">Verdict</th>
                <th scope="col">RTO</th>
                <th scope="col">RPO</th>
                <th scope="col">Data verified</th>
                <th scope="col">Reviewer</th>
            </tr>
        </thead>
        <tbody>
            {#each tests as test (test.test_id)}
                <tr data-test={test.test_id}>
                    <td class="mono" title={new Date(test.executed_at_ns / 1e6).toLocaleString()}>
                        {localDay(test.executed_at_ns)}
                    </td>
                    <td><a href={testHref(test)}>{test.experiment_name ?? test.evidence_id}</a></td>
                    <td><Badge tone="neutral" label={origin(test)} /></td>
                    <td class="nowrap">
                        {test.target_system ?? 'no target'}
                        {#if test.target_environment}<span class="dim">
                                · {test.target_environment}</span
                            >{/if}
                    </td>
                    <td>
                        <Badge
                            kind="verdict"
                            value={test.verdict}
                            label={verdictLabel(test.verdict)}
                        />
                    </td>
                    <td>
                        <DrObjectiveCell
                            measured={rtoText(test)}
                            target={test.rto_target_s}
                            alarm={rtoAlarm(test, test.rto_target_s)}
                        />
                    </td>
                    <td>
                        <DrObjectiveCell
                            measured={rpoText(test)}
                            target={test.rpo_target_s}
                            alarm={rpoAlarm(test, test.rpo_target_s)}
                        />
                    </td>
                    <td>{verified(test.data_verified)}</td>
                    <td>{test.reviewer ?? '—'}</td>
                </tr>
            {/each}
        </tbody>
    </table>
    <nav class="pager" aria-label="Pages of DR tests">
        <button disabled={!canPrev || busy} onclick={onprev}>Previous</button>
        <span>Page {page}: tests {first} to {last} of {total}</span>
        <button disabled={!canNext || busy} onclick={onnext}>Next</button>
    </nav>
{/if}

<style>
    /* Not the size of the global .dim: the size around it. */
    .dim {
        color: var(--text-faint);
        font-size: inherit;
    }
    .nowrap {
        white-space: nowrap;
    }
    .pager {
        display: flex;
        align-items: center;
        justify-content: flex-end;
        gap: var(--space-3);
        margin-top: var(--space-3);
        color: var(--text-dim);
        font-size: var(--font-size-xs);
    }
    .pager button {
        background: transparent;
        border: var(--border-width-sm) solid var(--border-strong);
        color: var(--text-dim);
        border-radius: var(--radius-xl);
        padding: var(--space-1) var(--space-3);
        font: inherit;
        cursor: pointer;
    }
    .pager button:hover:not(:disabled) {
        color: var(--text);
        border-color: var(--accent);
    }
    .pager button:disabled {
        opacity: 0.45;
        cursor: default;
    }
</style>
