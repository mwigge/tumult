<script lang="ts">
    // One chart of one target: its RTO or its RPO over the period. The part
    // around the charts explains their marks.
    import { hasPoints, recoveryOption } from '#lib/dr.js';
    import type { Objective, RecoveryGroup } from '#lib/dr.js';
    import type { ChartTheme } from '#lib/echarts.js';
    import EChart from './EChart.svelte';

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
        <div class="state">No {NAME[objective]} measured in this period.</div>
    {/if}
</div>

<style>
    h4 {
        margin: 0 0 4px;
        font-size: 11.5px;
        font-weight: 600;
        letter-spacing: 0.5px;
        color: var(--text-dim);
    }
    .state {
        padding: 20px;
    }
</style>
