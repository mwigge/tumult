<script lang="ts" module>
    export interface WfRow {
        span: Span;
        depth: number;
    }
</script>

<script lang="ts">
    // The signature span waterfall: ruler + indented tree + status-coloured
    // duration bars. Bars encode status (Ok=emerald, Error=red, Unset=slate);
    // the resilience.* span category is shown as a dim tag on the label.
    import type { Span } from '#lib/types.js';
    import { fmtDuration } from '#lib/api.js';

    let { spans, onselect }: { spans: Span[]; onselect: (span: Span) => void } = $props();

    interface Built {
        rows: WfRow[];
        min: number;
        spanNs: number;
    }

    const built: Built = $derived.by(() => {
        if (spans.length === 0) return { rows: [], min: 0, spanNs: 1 };

        const byId = new Map(spans.map((s) => [s.span_id, s]));
        // eslint-disable-next-line svelte/prefer-svelte-reactivity -- Local calculation; no retained reactive state.
        const children = new Map<string | null, Span[]>();
        const roots: Span[] = [];
        for (const s of spans) {
            const parent = s.parent_span_id && byId.has(s.parent_span_id) ? s.parent_span_id : null;
            if (parent === null) roots.push(s);
            else {
                const list = children.get(parent) ?? [];
                list.push(s);
                children.set(parent, list);
            }
        }
        const byStart = (a: Span, b: Span) => a.ts_ns - b.ts_ns;
        roots.sort(byStart);
        for (const list of children.values()) list.sort(byStart);

        const rows: WfRow[] = [];
        const visit = (s: Span, depth: number) => {
            rows.push({ span: s, depth });
            for (const c of children.get(s.span_id) ?? []) visit(c, depth + 1);
        };
        for (const r of roots) visit(r, 0);

        const min = Math.min(...spans.map((s) => s.ts_ns));
        const max = Math.max(...spans.map((s) => s.ts_ns + s.duration_ns));
        return { rows, min, spanNs: Math.max(max - min, 1) };
    });

    const ticks = $derived(Array.from({ length: 6 }, (_, i) => (built.spanNs / 5) * i));

    function barColor(span: Span): string {
        if (span.status_code === 'Error') return 'var(--fail)';
        if (span.status_code === 'Ok') return 'var(--ok)';
        return 'var(--border-strong)';
    }

    function tag(name: string): string {
        const parts = name.split('.');
        return parts.length > 1 ? parts.slice(1).join('.') : '';
    }

    function tickLabel(ns: number): string {
        const ms = ns / 1_000_000;
        if (ms < 1000) return `${Math.round(ms)}ms`;
        if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`;
        return `${Math.floor(ms / 60_000)}m${Math.round((ms % 60_000) / 1000)}s`;
    }
</script>

<div class="wf">
    <!-- Grid and tick labels share the lane box of the rows: a line stands
         under its label, inside the lane, from the ruler to the last row. -->
    <div class="grid" aria-hidden="true">
        {#each ticks as t (t)}
            <i style="left: {(t / built.spanNs) * 100}%"></i>
        {/each}
    </div>
    <div class="ruler">
        <div class="labels">
            {#each ticks as t (t)}
                <span style="left: {(t / built.spanNs) * 100}%">{tickLabel(t)}</span>
            {/each}
        </div>
    </div>

    {#each built.rows as row (row.span.span_id)}
        {@const s = row.span}
        {@const left = ((s.ts_ns - built.min) / built.spanNs) * 100}
        {@const width = Math.max((s.duration_ns / built.spanNs) * 100, 0.35)}
        <button
            class="row"
            onclick={() => onselect(s)}
            title="{s.span_name} — {fmtDuration(s.duration_ns)} ({s.status_code})"
        >
            <span class="name" style="padding-left: {row.depth * 16}px">
                {#if tag(s.span_name)}<em>{tag(s.span_name)}</em>{:else}{s.span_name}{/if}
            </span>
            <span class="lane">
                <span class="bar" style="left: {left}%; width: {width}%; background: {barColor(s)}"
                ></span>
            </span>
            <span class="dur mono">{fmtDuration(s.duration_ns)}</span>
        </button>
    {/each}
</div>

<style>
    .wf {
        /* The row columns; the lane is what is left between them. */
        --name-w: 240px;
        --dur-w: 66px;
        --gap: 10px;
        position: relative;
        font-size: var(--font-size-sm);
    }
    .ruler {
        position: sticky;
        top: 0;
        background: var(--bg-panel);
        z-index: var(--z-sticky);
        border-bottom: var(--border-width-sm) solid var(--border-strong);
        padding: 3px 0;
    }
    .ruler .labels {
        position: relative;
        height: 1.4em;
        margin: 0 calc(var(--dur-w) + var(--gap)) 0 calc(var(--name-w) + var(--gap));
        color: var(--text-faint);
        font-size: 10.5px;
        font-family: var(--mono);
    }
    .ruler .labels span {
        position: absolute;
        top: 0;
        transform: translateX(-50%);
        white-space: nowrap;
    }
    .grid {
        position: absolute;
        top: 0;
        bottom: 0;
        left: calc(var(--name-w) + var(--gap));
        right: calc(var(--dur-w) + var(--gap));
        z-index: var(--z-raised);
        pointer-events: none;
    }
    .grid i {
        position: absolute;
        top: 0;
        bottom: 0;
        width: 1px;
        transform: translateX(-50%);
        background: var(--border);
        opacity: 0.5;
    }
    .row {
        display: grid;
        grid-template-columns: var(--name-w) 1fr var(--dur-w);
        align-items: center;
        width: 100%;
        gap: var(--gap);
        background: transparent;
        border: none;
        border-bottom: var(--border-width-sm) solid
            color-mix(in srgb, var(--border) 55%, transparent);
        color: var(--text);
        padding: 3px 0;
        cursor: pointer;
        text-align: left;
        font: inherit;
    }
    .row:hover {
        background: var(--bg-hover);
    }
    .name {
        overflow: hidden;
        white-space: nowrap;
        text-overflow: ellipsis;
        color: var(--text-dim);
    }
    .name em {
        font-style: normal;
        color: var(--accent);
    }
    .lane {
        position: relative;
        height: 13px;
    }
    .bar {
        position: absolute;
        top: 2px;
        height: 9px;
        border-radius: var(--radius-xs);
        min-width: 2px;
    }
    .dur {
        color: var(--text-dim);
        text-align: right;
        font-size: var(--font-size-2xs);
    }
</style>
