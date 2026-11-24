<script lang="ts">
    // The colour legend of a chart: one line under it, with a small swatch
    // of each colour the chart draws next to the word for it. The chart is
    // a canvas; this line is text, so it reads without a pointer. `id` is
    // the id of the chart it explains.
    import type { LegendItem } from '#lib/legend.js';

    let { items, id }: { items: LegendItem[]; id?: string } = $props();
</script>

<div class="legend" data-legend={id}>
    {#each items as item (item.label)}
        <span class="item">
            {#if item.glyph}
                <b style:color={item.color}>{item.glyph}</b>
            {:else if item.mark === 'hatch'}
                <i class="swatch hatch"></i>
            {:else if item.color}
                <i class="swatch {item.mark ?? 'box'}" style:--swatch={item.color}></i>
            {/if}
            {item.label}
        </span>
    {/each}
</div>

<style>
    .legend {
        display: flex;
        flex-wrap: wrap;
        gap: 6px 16px;
        color: var(--text-dim);
        font-size: 11.5px;
        margin: 6px 0 10px;
    }
    .swatch {
        display: inline-block;
        width: 14px;
        height: 10px;
        vertical-align: middle;
        margin-right: 2px;
    }
    .swatch.box {
        background: var(--swatch);
        border: 1px solid var(--border-strong);
        border-radius: 2px;
    }
    .swatch.line,
    .swatch.dashed {
        height: 0;
        border-top: 2px solid var(--swatch);
    }
    .swatch.dashed {
        border-top-style: dashed;
    }
    .swatch.hatch {
        border: 1px solid var(--border-strong);
        background: repeating-linear-gradient(135deg, var(--text-dim) 0 1px, transparent 1px 4px);
    }
</style>
