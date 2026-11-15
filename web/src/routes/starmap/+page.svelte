<script lang="ts">
    // Star Map: one graph of everything Tumult knows, from the organisation
    // at the centre out to workloads, with the evidence, risk and
    // conformance lenses over the same nodes. Positions come from the pure
    // `layoutStar` and `spread`, so the map looks the same on every load. Range,
    // environment, lens, selection and focus live in the URL. The canvas
    // cannot be reached from the keyboard, so the same nodes are also a
    // list of buttons below it.
    import { onMount, tick } from 'svelte';
    import { page } from '$app/state';
    import { goto } from '$app/navigation';
    import { api } from '#lib/api.js';
    import type { StarEdge, StarKind, StarMap, StarNode } from '#lib/types.js';
    import RangeSwitch from '#lib/components/RangeSwitch.svelte';
    import EChart from '#lib/components/EChart.svelte';
    import StarDrawer from '#lib/components/StarDrawer.svelte';
    import { chartTheme, chartTokens } from '#lib/echarts.js';
    import type { ChartTheme, EChartsCoreOption } from '#lib/echarts.js';
    import { spread } from '#lib/starmap/force.js';
    import { layoutStar } from '#lib/starmap/layout.js';
    import type { Point } from '#lib/starmap/layout.js';
    import { listNodes } from '#lib/starmap/list.js';
    import { legendKinds, starSeries, statusKey } from '#lib/starmap/option.js';
    import { RANGES } from '#lib/ranges.js';
    import { tipFormatter } from '#lib/tooltip.js';
    import type { Tip } from '#lib/tooltip.js';
    import { LENSES, edgeTooltipHtml, spineTooltipHtml, tooltipHtml } from '#lib/starmap/style.js';
    import type { Lens, StyleContext } from '#lib/starmap/style.js';

    const CLUSTER = 'cluster:';
    /** Most buttons the node list renders at once; the filter narrows it. */
    const LISTED = 200;

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
    let filter = $state('');
    // Kinds switched off in the legend.
    let hidden: StarKind[] = $state([]);
    let listEl: HTMLElement | undefined = $state();
    let filterEl: HTMLInputElement | undefined = $state();

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
        const nodes = m.nodes.map((n) => ({ id: n.id, kind: n.kind, parent: n.parent }));
        const edges = m.edges.map((e) => ({ from: e.from, to: e.to }));
        return spread(nodes, edges, layoutStar(nodes, edges));
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
        return goto(`?${params}`, { replace: true, reset: false });
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
        data?: { raw?: StarNode | StarEdge | null; source?: string; target?: string };
    }

    function edgeTipOf(data: NonNullable<TipParams['data']>): Tip | null {
        if (data.raw) return edgeTooltipHtml(data.raw as StarEdge);
        if (!data.source || !data.target) return null;
        return spineTooltipHtml(data.target, data.source);
    }

    function tipOf(p: TipParams): Tip | null {
        if (!p.data) return null;
        if (p.dataType === 'edge') return edgeTipOf(p.data);
        return p.data.raw ? tooltipHtml(p.data.raw as StarNode, context) : null;
    }

    /** What the canvas is, for a reader who cannot see it. */
    const described = $derived.by(() => {
        if (!map) return '';
        const c = map.counts;
        return (
            `Star map of ${plural(c.nodes, 'node')} and ${plural(c.edges, 'edge')}: ` +
            `${c.risks_open} open risks, ${plural(c.gaps, 'gap')}. ` +
            'The node list below the map has every node.'
        );
    });

    const option: EChartsCoreOption = $derived.by(() => {
        if (!map || map.nodes.length === 0) return {};
        const series = starSeries({
            nodes: map.nodes,
            edges: map.edges,
            positions,
            context,
            hidden: new Set(hidden),
            focused: Boolean(focus),
            light: theme === 'tumult-light',
        });
        const t = context.tokens;
        return {
            tooltip: { ...t.tooltip, confine: true, formatter: tipFormatter(tipOf) },
            series: [series],
        };
    });

    const kinds = $derived(legendKinds(map?.nodes ?? []));

    function toggleKind(kind: StarKind) {
        hidden = hidden.includes(kind) ? hidden.filter((k) => k !== kind) : [...hidden, kind];
    }

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

    /** Select a node; a cluster has nothing of its own to show, so it expands. */
    function pick(n: StarNode) {
        if (n.kind === 'cluster') {
            setParams({ focus: focusId(n.id), sel: null });
            return;
        }
        setParams({ sel: n.id });
    }

    function onNode(params: NodeEvent) {
        const n = nodeOf(params);
        if (n) pick(n);
    }

    /** Close the drawer and hand the focus back to the node's button. */
    async function closeDrawer() {
        const id = sel;
        await setParams({ sel: null });
        await tick();
        const opener = listEl?.querySelector<HTMLElement>(`[data-node="${CSS.escape(id)}"]`);
        (opener ?? filterEl)?.focus();
    }

    function onNodeTwice(params: NodeEvent) {
        const n = nodeOf(params);
        if (n) setParams({ focus: focusId(n.id) });
    }

    function onRoam(view: { zoom: number }) {
        const next = view.zoom >= 2 ? 2 : view.zoom >= 1 ? 1 : 0;
        if (next !== detail) detail = next;
    }

    function plural(n: number, word: string) {
        return `${n} ${word}${n === 1 ? '' : 's'}`;
    }

    const matching = $derived(listNodes(map?.nodes ?? [], filter));
    const listed = $derived(matching.slice(0, LISTED));
    const kindOf = (n: StarNode) => (n.kind === 'org' && n.org_kind ? n.org_kind : n.kind);

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
            <div class="skeleton" style="height: 720px"></div>
        {:else if map.nodes.length === 0}
            <div class="state">
                {#if map.focus_found === false}
                    <span class="mono">{focus}</span> is unknown or out of scope.
                {:else if focus}
                    Nothing is visible around this node.
                {:else}
                    Nothing was seen in this window.
                {/if}
            </div>
        {:else}
            <EChart
                id="starmap"
                label={described}
                {option}
                height={720}
                merge
                viewKey={focus}
                onclick={onNode}
                ondblclick={onNodeTwice}
                onroam={onRoam}
            />
            <p class="sub">
                Hover to see a node and its links. Click for evidence. Double-click to focus.
            </p>
            <p class="sub legend" data-testid="starmap-legend">
                risks: {map.counts.risks_open} open, {map.counts.risks_suppressed} suppressed ·
                {plural(map.counts.gaps, 'gap')}
                {#if map.counts.risks_unplaced > 0}
                    · {plural(map.counts.risks_unplaced, 'risk')} not on the map
                {/if}
                {#if map.counts.clustered > 0}
                    · {plural(map.counts.clustered, 'node')} collapsed into clusters
                {/if}
                {#if map.counts.pruned > 0}
                    · {plural(map.counts.pruned, 'node')} outside the focus
                {/if}
                {#if map.truncated}
                    · incomplete: a source table is longer than its row limit
                {/if}
            </p>
            <div class="kinds" role="group" aria-label="node kinds" data-testid="starmap-kinds">
                {#each kinds as k (k.kind)}
                    <button
                        class="chip"
                        aria-pressed={!hidden.includes(k.kind)}
                        title="Show or hide this kind"
                        onclick={() => toggleKind(k.kind)}
                    >
                        <span class="dot" style="background: {k.color}"></span>
                        {k.name}
                        <span class="count">{k.count}</span>
                    </button>
                {/each}
            </div>
            <ul class="key" aria-label="how to read the map" data-testid="starmap-key">
                {#each statusKey(lens) as k (k.status)}
                    <li><span class="ring {k.status}"></span>{k.mark} {k.text}</li>
                {/each}
                <li><span class="ring missing"></span>no evidence</li>
                <li><span class="ring stale"></span>stale evidence</li>
                <li><span class="ring suppressed"></span>only suppressed risks</li>
                <li><span class="line arrow"></span><span class="mono">NAME</span> relation</li>
                <li><span class="line"></span>sits under</li>
            </ul>
            {#if unavailable.length > 0}
                <p class="sub">Not available yet: {unavailable.join(', ')}.</p>
            {/if}
            <p class="sub">{map.disclaimer}</p>
            <section class="nodes" aria-label="nodes" bind:this={listEl}>
                <div class="nodes-head">
                    <h2>Nodes</h2>
                    <input
                        type="search"
                        aria-label="Filter nodes"
                        placeholder="filter by name, id or kind"
                        bind:value={filter}
                        bind:this={filterEl}
                    />
                    <span class="sub" aria-live="polite">
                        {listed.length} of {matching.length}
                        {#if matching.length > listed.length}· filter to see the rest{/if}
                    </span>
                </div>
                <ul>
                    {#each listed as n (n.id)}
                        <li>
                            <button
                                data-node={n.id}
                                aria-pressed={n.id === sel}
                                title={n.id}
                                onclick={() => pick(n)}
                            >
                                <span class="kind">{kindOf(n)}</span>
                                {n.label}
                            </button>
                        </li>
                    {/each}
                </ul>
            </section>
        {/if}
    </div>
    {#if selected}
        <StarDrawer
            node={selected}
            {range}
            onclose={closeDrawer}
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
    .kinds {
        display: flex;
        flex-wrap: wrap;
        gap: 6px;
        margin-top: 8px;
    }
    .chip {
        display: inline-flex;
        align-items: center;
        gap: 6px;
        background: var(--bg-panel);
        border: 1px solid var(--border-strong);
        border-radius: 999px;
        color: var(--text);
        cursor: pointer;
        font-size: 12px;
        padding: 2px 10px 2px 6px;
    }
    .chip[aria-pressed='false'] {
        opacity: 0.45;
        text-decoration: line-through;
    }
    .chip .dot {
        width: 12px;
        height: 12px;
        border-radius: 50%;
    }
    .chip .count {
        color: var(--text-dim);
        font-size: 11px;
    }
    .key {
        list-style: none;
        display: flex;
        flex-wrap: wrap;
        gap: 4px 16px;
        margin: 8px 0 0;
        padding: 0;
        color: var(--text-dim);
        font-size: 12px;
    }
    .key li {
        display: inline-flex;
        align-items: center;
        gap: 6px;
    }
    .ring {
        box-sizing: border-box;
        width: 14px;
        height: 14px;
        border-radius: 50%;
        background: var(--border-strong);
        border: 2px solid var(--ok);
    }
    .ring.warn {
        border: 3px solid var(--warn);
    }
    .ring.fail {
        border: 4px solid var(--fail);
    }
    .ring.missing {
        background: transparent;
        border: 2px dashed var(--text-dim);
    }
    .ring.stale {
        border: 2px dashed var(--warn);
    }
    .ring.suppressed {
        border: 2px dotted var(--text-dim);
        opacity: 0.45;
    }
    .line {
        position: relative;
        width: 22px;
        border-top: 1.5px solid var(--text-dim);
    }
    .line.arrow::after {
        content: '';
        position: absolute;
        right: -1px;
        top: -5px;
        border: 4px solid transparent;
        border-left: 7px solid var(--text-dim);
        border-right: 0;
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
    .nodes {
        margin-top: 14px;
        border-top: 1px solid var(--border);
        padding-top: 10px;
    }
    .nodes-head {
        display: flex;
        gap: 10px;
        align-items: center;
        flex-wrap: wrap;
    }
    .nodes-head h2 {
        font-size: 12.5px;
    }
    .nodes-head .sub {
        margin: 0;
    }
    .nodes ul {
        list-style: none;
        margin: 8px 0 0;
        padding: 0;
        display: flex;
        flex-wrap: wrap;
        gap: 6px;
        max-height: 180px;
        overflow-y: auto;
    }
    .nodes button {
        background: var(--bg-panel);
        border: 1px solid var(--border-strong);
        border-radius: 4px;
        color: var(--text);
        cursor: pointer;
        font-size: 12px;
        padding: 3px 8px;
    }
    .nodes button[aria-pressed='true'] {
        border-color: var(--accent);
        background: var(--bg-hover);
    }
    .nodes .kind {
        color: var(--text-dim);
        font-size: 11px;
        margin-right: 4px;
    }
</style>
