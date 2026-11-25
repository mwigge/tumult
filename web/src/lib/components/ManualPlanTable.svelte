<script lang="ts">
    // The planned manual exercises, the soonest due first as the API lists
    // them. A plan still planned after its due date is marked overdue. A row
    // opens to the detail the page puts under it.
    import type { Snippet } from 'svelte';
    import { isOverdue, localDay } from '#lib/manual.js';
    import type { ManualPlan } from '#lib/types.js';
    import Badge from './Badge.svelte';

    let {
        plans,
        nowNs,
        canWrite,
        openId = null,
        onopen,
        onedit,
        oncancel,
        detail,
    }: {
        plans: ManualPlan[];
        nowNs: number;
        canWrite: boolean;
        openId?: string | null;
        onopen?: (plan: ManualPlan) => void;
        onedit?: (plan: ManualPlan) => void;
        oncancel?: (plan: ManualPlan) => void;
        detail?: Snippet<[ManualPlan]>;
    } = $props();

    const secondsText = (s: number | null) => (s === null ? '—' : `${s} s`);
</script>

{#if plans.length === 0}
    <div class="state">No plans yet.</div>
{:else}
    <table class="data">
        <thead>
            <tr>
                <th>Plan</th><th>Type</th><th>Target</th><th>Owner</th><th>Due</th><th>Status</th>
                <th>Repeats</th><th>RTO / RPO</th><th></th>
            </tr>
        </thead>
        <tbody>
            {#each plans as plan (plan.id)}
                <tr class="clickable" data-plan={plan.id} onclick={() => onopen?.(plan)}>
                    <td>{plan.name}</td>
                    <td><Badge tone="neutral" label={plan.exercise_type} /></td>
                    <td>
                        {plan.target_system ?? '—'}
                        {#if plan.target_environment}<span class="dim">
                                · {plan.target_environment}</span
                            >{/if}
                    </td>
                    <td>{plan.owner}</td>
                    <td class="nowrap">
                        <span class="mono">{localDay(plan.due_at_ns)}</span>
                        {#if isOverdue(plan, nowNs)}<Badge tone="fail" label="overdue" />{/if}
                    </td>
                    <td><Badge kind="plan" value={plan.status} /></td>
                    <td>{plan.interval_days ? `every ${plan.interval_days} days` : 'once'}</td>
                    <td class="mono">
                        {secondsText(plan.rto_target_s)} / {secondsText(plan.rpo_target_s)}
                    </td>
                    <td>
                        <div class="row-actions">
                            {#if canWrite && plan.status === 'planned'}
                                <button
                                    class="btn"
                                    aria-label="Edit plan {plan.name}"
                                    onclick={(e) => {
                                        e.stopPropagation();
                                        onedit?.(plan);
                                    }}>Edit</button
                                >
                                <button
                                    class="btn danger"
                                    aria-label="Cancel plan {plan.name}"
                                    onclick={(e) => {
                                        e.stopPropagation();
                                        oncancel?.(plan);
                                    }}>Cancel</button
                                >
                            {/if}
                        </div>
                    </td>
                </tr>
                {#if openId === plan.id && detail}
                    <tr class="detail-row">
                        <td colspan="9">{@render detail(plan)}</td>
                    </tr>
                {/if}
            {/each}
        </tbody>
    </table>
{/if}

<style>
    .row-actions {
        display: flex;
        gap: var(--space-1-5);
        justify-content: flex-end;
    }
    .dim {
        color: var(--text-faint);
    }
    .nowrap {
        white-space: nowrap;
        /* The overdue badge after the day keeps a gap. */
        --badge-gap: var(--space-1-5);
    }
</style>
