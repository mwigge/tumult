<script lang="ts">
    import type { Kpi } from '#lib/types.js';
    import { fmtDelta, fmtKpi } from '#lib/api.js';
    import Sparkline from './Sparkline.svelte';

    let { kpi }: { kpi: Kpi } = $props();
    const delta = $derived(fmtDelta(kpi.delta, kpi.unit));
    const sparkColor = $derived(
        kpi.name === 'pass_rate'
            ? 'var(--ok)'
            : kpi.name === 'deviation_rate'
              ? 'var(--warn)'
              : 'var(--accent)',
    );
</script>

<div class="kpi panel">
    <div class="label">{kpi.label}</div>
    <div class="row">
        <div>
            <span class="value mono">{fmtKpi(kpi.value, kpi.unit)}</span>
            {#if delta.text}
                <span
                    class="delta {delta.cls}"
                    title="vs previous {kpi.unit === 'ratio' ? 'window' : 'window'}"
                    >{delta.text}</span
                >
            {/if}
        </div>
        <Sparkline points={kpi.spark} color={sparkColor} />
    </div>
</div>

<style>
    .kpi {
        min-width: 0;
    }
    .label {
        color: var(--text-dim);
        font-size: var(--font-size-2xs);
        text-transform: uppercase;
        letter-spacing: var(--letter-spacing-md);
        margin-bottom: var(--space-1-5);
    }
    .row {
        display: flex;
        justify-content: space-between;
        align-items: flex-end;
        gap: var(--space-2);
    }
    .value {
        font-size: 26px;
        font-weight: var(--font-weight-strong);
    }
    .delta {
        font-size: var(--font-size-xs);
        margin-left: var(--space-2);
    }
</style>
