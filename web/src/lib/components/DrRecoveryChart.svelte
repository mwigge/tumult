<script lang="ts">
    // One chart of one target: its RTO or its RPO over the period. The part
    // around the charts explains their marks.
    import { hasPoints, recoveryOption } from '#lib/dr.js';
    import type { Objective, RecoveryGroup } from '#lib/dr.js';
    import type { ChartTheme } from '#lib/echarts.js';
    import EChart from './EChart.svelte';
    import State from './State.svelte';

    let {
        group,
        objective,
        period,
        theme,
        id,
    }: {
        group: RecoveryGroup;
        objective: Objective;
        period: { from_ns: number; to_ns: number };
        theme: ChartTheme;
        id: string;
    } = $props();

    const NAME: Record<Objective, string> = { rto: 'RTO', rpo: 'RPO' };

    const option = $derived(recoveryOption(group, objective, period, theme));
    const label = $derived(
        `${NAME[objective]} of ${group.system ?? 'no target'}` +
            `${group.environment ? ` in ${group.environment}` : ''} over the period`,
    );
</script>

<div class="chart">
    <h4>{NAME[objective]}</h4>
    {#if hasPoints(group, objective)}
        <EChart {option} height={180} {id} {label} />
    {:else}
        <State kind="empty" style="padding: var(--space-5)"
            >No {NAME[objective]} measured in this period.</State
        >
    {/if}
</div>

<style>
    h4 {
        margin: 0 0 var(--space-1);
        font-size: var(--font-size-xs);
        font-weight: var(--font-weight-strong);
        letter-spacing: var(--letter-spacing-sm);
        color: var(--text-dim);
    }
</style>
