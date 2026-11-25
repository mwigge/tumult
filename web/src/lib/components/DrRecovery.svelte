<script lang="ts">
    // The RTO and the RPO of each target over the period: one part per
    // target, with a chart of each, and one legend in words for them all.
    import { onMount } from 'svelte';
    import { CHART_LIMIT, MANUAL_COLOR } from '#lib/dr.js';
    import type { RecoveryGroup } from '#lib/dr.js';
    import { chartTheme, chartTokens, followTheme } from '#lib/echarts.js';
    import type { LegendItem } from '#lib/legend.js';
    import ChartLegend from './ChartLegend.svelte';
    import DrRecoveryChart from './DrRecoveryChart.svelte';

    let {
        groups,
        period,
        truncated,
    }: {
        groups: RecoveryGroup[];
        period: { from_ns: number; to_ns: number };
        /** The period has more tests than the charts read. */
        truncated: boolean;
    } = $props();

    // The charts are drawn in the colours of the theme in force.
    let theme = $state(chartTheme());
    onMount(() => followTheme((next) => (theme = next)));

    const key = (group: RecoveryGroup) => `${group.system ?? '-'}/${group.environment ?? '-'}`;

    const legend = $derived.by((): LegendItem[] => {
        const t = chartTokens(theme);
        return [
            { label: 'measured', color: t.accent, mark: 'line' },
            { label: 'manual test', glyph: '◆', color: MANUAL_COLOR },
            { label: 'did not recover: the RTO was at least this', glyph: '▲', color: t.fail },
            { label: 'target', color: t.fail, mark: 'dashed' },
        ];
    });
</script>

{#if groups.length === 0}
    <div class="state">No DR test in this period to draw.</div>
{:else}
    <ChartLegend id="dr-recovery" items={legend} />
    {#if truncated}
        <p class="note">The charts show the newest {CHART_LIMIT} tests of the period.</p>
    {/if}
    {#each groups as group, i (key(group))}
        <section class="target" aria-labelledby="dr-target-{i}" data-target={key(group)}>
            <h3 id="dr-target-{i}">
                {group.system ?? 'no target'}{group.environment ? ` · ${group.environment}` : ''}
            </h3>
            <div class="grid cols-2">
                <DrRecoveryChart {group} objective="rto" {period} {theme} id="dr-rto-{i}" />
                <DrRecoveryChart {group} objective="rpo" {period} {theme} id="dr-rpo-{i}" />
            </div>
        </section>
    {/each}
{/if}

<style>
    .target + .target {
        border-top: var(--border-width-sm) solid var(--border);
        margin-top: var(--space-3);
        padding-top: var(--space-3);
    }
    h3 {
        font-size: var(--font-size-md);
        margin-bottom: var(--space-1-5);
    }
    .note {
        color: var(--text-dim);
        font-size: var(--font-size-xs);
        margin: 0 0 var(--space-2);
    }
</style>
