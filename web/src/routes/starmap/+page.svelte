<script lang="ts">
    // Star Map: one graph of everything Tumult knows, from the organisation
    // at the centre out to workloads, with the evidence, risk and
    // conformance lenses over the same nodes. Positions come from the pure
    // `layoutStar`, so the map looks the same on every load. Range,
    // environment, lens, selection and focus live in the URL.
    import { onMount } from 'svelte';
    import { page } from '$app/state';
    import { goto } from '$app/navigation';
    import { api } from '#lib/api.js';
    import type { StarEdge, StarMap, StarNode } from '#lib/types.js';
    import RangeSwitch from '#lib/components/RangeSwitch.svelte';
    import EChart from '#lib/components/EChart.svelte';
    import StarDrawer from '#lib/components/StarDrawer.svelte';
    import { chartTheme, chartTokens } from '#lib/echarts.js';
    import type { ChartTheme, EChartsCoreOption } from '#lib/echarts.js';
    import { layoutStar } from '#lib/starmap/layout.js';
    import type { Point } from '#lib/starmap/layout.js';
    import {
        KINDS,
        LENSES,
        edgeTooltipHtml,
        toItem,
        toLink,
        tooltipHtml,
    } from '#lib/starmap/style.js';
    import type { Lens, StarItem, StarLink, StyleContext } from '#lib/starmap/style.js';

    const RANGES = ['24h', '7d', '14d', '30d'];
    const CLUSTER = 'cluster:';

    const param = (key: string) => page.url.searchParams.get(key) ?? '';
    const range = $derived(param('range') || '7d');
    const env = $derived(param('env'));
    const focus = $derived(param('focus'));
    const depth = $derived(param('depth'));
    const sel = $derived(param('sel'));
    const lens: Lens = $derived(LENSES.find((l) => l === param('lens')) ?? 'evidence');

    let map = $state<StarMap | null>(null);
    let error = $state<string | null>(null);
    let loading = $state(false);
    let positions: Map<string, Point> = $state.raw(new Map());
    // Environments seen so far, so the filter keeps its choices once one is set.
    let envs: string[] = $state([]);
    let theme: ChartTheme = $state(chartTheme());
    // Label level of detail: 0 below zoom 1, 1 from zoom 1, 2 from zoom 2.
    let detail = $state(1);

    onMount(() => {
        const retheme = () => (theme = chartTheme());
        const mo = new MutationObserver(retheme);
        mo.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });
        const mq = window.matchMedia?.('(prefers-color-scheme: light)');
        mq?.addEventListener?.('change', retheme);
        return () => {
            mo.disconnect();
            mq?.removeEventListener?.('change', retheme);
        };
    });

    function place(m: StarMap): Map<string, Point> {
        return layoutStar(
            m.nodes.map((n) => ({ id: n.id, kind: n.kind, parent: n.parent })),
            m.edges.map((e) => ({ from: e.from, to: e.to })),
        );
    }

    function remember(m: StarMap) {
        const seen = [...envs, m.env, ...m.nodes.map((n) => n.env)];
        const known = seen.filter((e): e is string => Boolean(e));
        envs = known.filter((e, i) => known.indexOf(e) === i).sort();
    }

    $effect(() => {
        const params = { range, env, focus, depth: focus ? depth : '' };
        let cancelled = false;
        loading = true;
        error = null;
        api.starmap(params)
            .then((m) => {
                if (cancelled) return;
                positions = place(m);
                remember(m);
                map = m;
                loading = false;
            })
            .catch((e: unknown) => {
                if (cancelled) return;
                error = String(e);
                loading = false;
            });
        return () => {
            cancelled = true;
        };
    });

    function setParams(changes: Record<string, string | null>) {
        // eslint-disable-next-line svelte/prefer-svelte-reactivity -- Local calculation; no retained reactive state.
        const params = new URLSearchParams(page.url.searchParams.toString());
        for (const [key, value] of Object.entries(changes)) {
            if (value) params.set(key, value);
            else params.delete(key);
        }
        goto(`?${params}`, { replace: true, reset: false });
    }

    const selected: StarNode | null = $derived(map?.nodes.find((n) => n.id === sel) ?? null);

    const context: StyleContext = $derived({
        lens,
        tokens: chartTokens(theme),
        selected: sel || null,
        zoom: detail,
        nowNs: map?.as_of_ns ?? 0,
    });

    interface TipParams {
        dataType?: string;
        data?: { raw?: StarNode | StarEdge };
    }

    function tip(p: TipParams): string {
        const raw = p.data?.raw;
        if (!raw) return '';
        if (p.dataType === 'edge') return edgeTooltipHtml(raw as StarEdge);
        return tooltipHtml(raw as StarNode, context);
    }

    const option: EChartsCoreOption = $derived.by(() => {
        if (!map || map.nodes.length === 0) return {};
        const t = context.tokens;
        const data: StarItem[] = [];
        for (const n of map.nodes) data.push(toItem(n, positions.get(n.id) ?? [0, 0], context));
        const links: StarLink[] = [];
        for (const e of map.edges) links.push(toLink(e, context));
        return {
            tooltip: { ...t.tooltip, confine: true, formatter: tip },
            series: [
                {
                    id: 'star',
                    type: 'graph',
                    layout: 'none',
                    roam: true,
                    scaleLimit: { min: 0.2, max: 8 },
                    nodeScaleRatio: 0.4,
                    edgeSymbol: ['none', 'arrow'],
                    edgeSymbolSize: 5,
                    emphasis: {
                        focus: 'adjacency',
                        label: { show: true },
                        lineStyle: { width: 2, opacity: 0.9 },
                    },
                    blur: {
                        itemStyle: { opacity: 0.12 },
                        lineStyle: { opacity: theme === 'tumult-light' ? 0.12 : 0.05 },
                        label: { show: false },
                    },
                    select: { itemStyle: { borderColor: t.accent, borderWidth: 3 } },
                    label: {
                        show: true,
                        position: 'right',
                        color: t.text,
                        fontSize: 11,
                        formatter: (p: { data?: { raw?: StarNode } }) => p.data?.raw?.label ?? '',
                    },
                    labelLayout: { hideOverlap: true },
                    categories: KINDS.map((k) => ({ name: k })),
                    lineStyle: { color: t.axis, opacity: 0.5, curveness: 0.15 },
                    data,
                    links,
                    animation: false,
                },
            ],
        };
    });

    interface NodeEvent {
        dataType?: string;
        data?: unknown;
    }

    function nodeOf(params: NodeEvent): StarNode | null {
        if (params.dataType !== 'node') return null;
        return (params.data as { raw?: StarNode } | undefined)?.raw ?? null;
    }

    /** A cluster stands for the nodes below its home; focusing there expands it. */
    const focusId = (id: string) => (id.startsWith(CLUSTER) ? id.slice(CLUSTER.length) : id);

    function onNode(params: NodeEvent) {
        const n = nodeOf(params);
        if (!n) return;
        if (n.kind === 'cluster') {
            setParams({ focus: focusId(n.id), sel: null });
            return;
        }
        setParams({ sel: n.id });
    }

    function onNodeTwice(params: NodeEvent) {
        const n = nodeOf(params);
        if (n) setParams({ focus: focusId(n.id) });
    }

    function onRoam(view: { zoom: number }) {
        const next = view.zoom >= 2 ? 2 : view.zoom >= 1 ? 1 : 0;
        if (next !== detail) detail = next;
    }

    const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? '' : 's'}`;

    const unavailable = $derived(
        Object.entries(map?.sources ?? {})
            .filter(([, state]) => state === 'unavailable')
            .map(([name]) => name),
    );
</script>

<div class="page-head">
    <h1>Star Map</h1>
    <span class="sub">
        {map ? `${plural(map.counts.nodes, 'node')} · ${plural(map.counts.edges, 'edge')}` : ''}
        {loading && map ? '· loading' : ''}
    </span>
    <div class="controls">
        <div class="seg" role="group" aria-label="lens">
            {#each LENSES as l (l)}
                <button class:active={lens === l} onclick={() => setParams({ lens: l })}>
                    {l}
                </button>
            {/each}
        </div>
        <select
            aria-label="environment"
            value={env}
            onchange={(e) => setParams({ env: e.currentTarget.value, sel: null })}
        >
            <option value="">all environments</option>
            {#each env && !envs.includes(env) ? [env, ...envs] : envs as e (e)}
                <option value={e}>{e}</option>
            {/each}
        </select>
        <RangeSwitch value={range} ranges={RANGES} onchange={(r) => setParams({ range: r })} />
    </div>
</div>

{#if focus}
    <p class="focus">
        Showing the neighbourhood of <span class="mono">{focus}</span>
        <button onclick={() => setParams({ focus: null, depth: null })} aria-label="Clear focus">
            show everything
        </button>
    </p>
{/if}

<div class="layout">
    <div class="panel map">
        {#if error}
            <div class="state error">Failed to load the star map: {error}</div>
        {:else if !map}
            <div class="skeleton" style="height: 640px"></div>
        {:else if map.nodes.length === 0}
            <div class="state">
                {focus
                    ? 'Nothing is visible around this node.'
                    : 'Nothing was seen in this window.'}
            </div>
        {:else}
            <EChart
                id="starmap"
                {option}
                height={640}
                merge
                onclick={onNode}
                ondblclick={onNodeTwice}
                onroam={onRoam}
            />
            <p class="sub legend" data-testid="starmap-legend">
                risks: {map.counts.risks_open} open, {map.counts.risks_suppressed} suppressed ·
                {plural(map.counts.gaps, 'gap')}
                {#if map.counts.clustered > 0}
                    · {plural(map.counts.clustered, 'node')} collapsed into clusters
                {/if}
                {#if map.counts.pruned > 0}
                    · {plural(map.counts.pruned, 'node')} outside the focus
                {/if}
            </p>
            <p class="sub">
                shape is the kind of node · a dashed hollow node has no evidence, a thin amber dash
                is stale evidence · a dimmed node has only suppressed risks · click a node for its
                evidence, double-click to focus on it
            </p>
            {#if unavailable.length > 0}
                <p class="sub">Not available yet: {unavailable.join(', ')}.</p>
            {/if}
            <p class="sub">{map.disclaimer}</p>
        {/if}
    </div>
    {#if selected}
        <StarDrawer
            node={selected}
            {range}
            onclose={() => setParams({ sel: null })}
            onfocus={(id) => setParams({ focus: focusId(id) })}
        />
    {/if}
</div>

<style>
    .controls {
        margin-left: auto;
        display: flex;
        gap: 8px;
        align-items: center;
        flex-wrap: wrap;
    }
    select {
        background: var(--bg-panel);
        color: var(--text);
        border: 1px solid var(--border-strong);
        border-radius: 4px;
        padding: 4px 8px;
        font-size: 12.5px;
    }
    .layout {
        display: flex;
        gap: 14px;
        align-items: flex-start;
        flex-wrap: wrap;
    }
    .map {
        flex: 1 1 520px;
        min-width: 0;
    }
    .sub {
        color: var(--text-dim);
        font-size: 12.5px;
        margin: 8px 0 0;
    }
    .focus {
        color: var(--text-dim);
        font-size: 12.5px;
        margin: 0 0 10px;
    }
    .focus button {
        background: transparent;
        border: none;
        color: var(--accent);
        cursor: pointer;
        font-size: 12.5px;
        padding: 0 0 0 8px;
    }
    .mono {
        font-family: var(--mono);
    }
</style>
