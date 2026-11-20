<script lang="ts">
    // Org hierarchy rollups: root = KPI cards + treemap; drill by click with
    // breadcrumb + ?node= URL state; non-root = indented tree table.
    // `?view=dora` shows DORA coverage and the maturity level instead.
    import { page } from '$app/state';
    import { goto } from '$app/navigation';
    import { api } from '#lib/api.js';
    import type { EChartsCoreOption } from '#lib/echarts.js';
    import { CHART } from '#lib/echarts.js';
    import {
        bandColor,
        bandGlyph,
        bandLabel,
        deltaClass,
        fmtDelta,
        fmtScore,
        tileLabel,
        tileScore,
    } from '#lib/org-score.js';
    import { COMPLIANCE_SCORE, COMPLIANCE_SCORE_LINE } from '#lib/score-names.js';
    import { strong, tip, tipFormatter } from '#lib/tooltip.js';
    import type { OrgNodeScore, ScoreTree, SparkPoint } from '#lib/types.js';
    import EChart from '#lib/components/EChart.svelte';
    import Sparkline from '#lib/components/Sparkline.svelte';
    import RangeSwitch from '#lib/components/RangeSwitch.svelte';
    import DoraScore from '#lib/components/DoraScore.svelte';

    const node = $derived(page.url.searchParams.get('node') ?? '');
    const range = $derived(page.url.searchParams.get('range') ?? '7d');

    const view = $derived(page.url.searchParams.get('view') === 'dora' ? 'dora' : 'org');
    const lei = $derived(page.url.searchParams.get('lei') ?? '');

    let tree: ScoreTree | null = $state(null);
    let error: string | null = $state(null);

    function setView(next: 'org' | 'dora') {
        // eslint-disable-next-line svelte/prefer-svelte-reactivity -- Local calculation; no retained reactive state.
        const params = new URLSearchParams(page.url.searchParams.toString());
        if (next === 'dora') params.set('view', 'dora');
        else params.delete('view');
        goto(`?${params}`, { reset: false });
    }

    function setLei(next: string) {
        // eslint-disable-next-line svelte/prefer-svelte-reactivity -- Local calculation; no retained reactive state.
        const params = new URLSearchParams(page.url.searchParams.toString());
        if (next) params.set('lei', next);
        else params.delete('lei');
        goto(`?${params}`, { replace: true, reset: false });
    }

    $effect(() => {
        const n = node;
        const r = range;
        if (view === 'dora') return;
        let cancelled = false;
        tree = null;
        error = null;
        api.scoreTree(n, r)
            .then((t) => !cancelled && (tree = t))
            .catch((e) => !cancelled && (error = String(e)));
        return () => {
            cancelled = true;
        };
    });

    function drill(path: string) {
        // eslint-disable-next-line svelte/prefer-svelte-reactivity -- Local calculation; no retained reactive state.
        const params = new URLSearchParams(page.url.searchParams.toString());
        if (path) params.set('node', path);
        else params.delete('node');
        goto(`?${params}`, { reset: false });
    }

    function setRange(r: string) {
        // eslint-disable-next-line svelte/prefer-svelte-reactivity -- Local calculation; no retained reactive state.
        const params = new URLSearchParams(page.url.searchParams.toString());
        params.set('range', r);
        goto(`?${params}`, { replace: true, reset: false });
    }

    // Breadcrumb segments for the current node path.
    const crumbs = $derived(node === '' ? [] : node.split('/'));

    let treemapOption: EChartsCoreOption | null = $state(null);
    /** What a tile of the treemap carries for its tooltip. */
    interface TreeTip {
        path?: string;
        score?: number | null;
        band?: string;
        scored?: number;
        expected?: number;
    }

    $effect(() => {
        if (!tree || node !== '') {
            treemapOption = null;
            return;
        }
        const children: OrgNodeScore[] = tree.children;
        treemapOption = {
            tooltip: {
                ...CHART.tooltip,
                formatter: tipFormatter((p: { data?: TreeTip }) => {
                    const d = p.data ?? {};
                    return tip(
                        strong(d.path),
                        tileScore(d.score ?? null, d.band ?? ''),
                        `coverage ${d.scored ?? 0}/${d.expected ?? 0}`,
                    );
                }),
            },
            series: [
                {
                    type: 'treemap',
                    roam: false,
                    nodeClick: false,
                    breadcrumb: { show: false },
                    width: '100%',
                    height: '100%',
                    label: {
                        show: true,
                        formatter: (p: { name?: string; data?: { score?: number | null } }) =>
                            tileLabel(p.name ?? '', p.data?.score ?? null),
                        color: '#0b0e11',
                        fontWeight: 600,
                        fontSize: 12,
                    },
                    itemStyle: { borderColor: '#0b0e11', borderWidth: 2, gapWidth: 3 },
                    emphasis: { label: { color: '#0b0e11' } },
                    data: children.map((c: OrgNodeScore) => ({
                        name: c.name,
                        // Area ∝ Σ criticality weights of the subtree's leaves; a tiny
                        // floor keeps empty subtrees visible.
                        value: Math.max(c.weight, 0.2),
                        path: c.path,
                        score: c.score,
                        band: c.band,
                        scored: c.scored,
                        expected: c.expected,
                        itemStyle: { color: bandColor(c.band) },
                    })),
                },
            ],
        };
    });

    function onTreemapClick(params: { data?: unknown }) {
        const d = params.data as { path?: string } | undefined;
        if (d?.path) drill(d.path);
    }

    let sparkPoints: SparkPoint[] = $state([]);
    $effect(() => {
        const t = tree;
        sparkPoints = t ? t.sparkline.map((p: [number, number]) => ({ ts: p[0], v: p[1] })) : [];
    });
</script>

<div class="page-head">
    <h1>Scores</h1>
    <div class="views" role="group" aria-label="score view">
        <button
            class:on={view === 'org'}
            aria-pressed={view === 'org'}
            onclick={() => setView('org')}>Org scores</button
        >
        <button
            class:on={view === 'dora'}
            aria-pressed={view === 'dora'}
            onclick={() => setView('dora')}>DORA and maturity</button
        >
    </div>
    {#if view === 'org'}
        <nav class="crumbs" aria-label="org breadcrumb">
            <button class:current={node === ''} onclick={() => drill('')}>company</button>
            {#each crumbs as seg, i (i)}
                <span class="sep">/</span>
                <button
                    class:current={i === crumbs.length - 1}
                    onclick={() => drill(crumbs.slice(0, i + 1).join('/'))}>{seg}</button
                >
            {/each}
        </nav>
        <div class="controls">
            <RangeSwitch value={range} onchange={setRange} />
        </div>
    {/if}
</div>

{#if view === 'dora'}
    <DoraScore {lei} onlei={setLei} />
{:else if error}
    <div class="panel"><div class="state error">Failed to load org scores: {error}</div></div>
{:else if !tree}
    <div class="panel"><div class="skeleton" style="height: 320px"></div></div>
{:else}
    <div class="cards">
        <div class="panel card">
            <div class="label">Score — {tree.name}</div>
            <div class="value mono" style="color: {bandColor(tree.band)}">
                <span class="glyph">{bandGlyph(tree.band)}</span>{fmtScore(tree.score)}
            </div>
            <div class="hint">{bandLabel(tree.band)}</div>
        </div>
        <div class="panel card">
            <div class="label">Δ vs previous {range}</div>
            <div class="value mono delta {deltaClass(tree.delta)}">{fmtDelta(tree.delta)}</div>
            <div class="hint">period-over-period</div>
        </div>
        <div class="panel card">
            <div class="label">Coverage</div>
            <div class="value mono">{tree.scored}/{tree.expected}</div>
            <div class="hint">{(tree.coverage * 100).toFixed(0)}% of expected evidence scored</div>
        </div>
        <div class="panel card">
            <div class="label">Weakest member</div>
            <div class="value weakest">{tree.weakest ?? '—'}</div>
            <div class="hint"><Sparkline points={sparkPoints} color={bandColor(tree.band)} /></div>
        </div>
    </div>
    <p class="meaning" data-testid="score-meaning">
        <strong>{COMPLIANCE_SCORE}.</strong>
        {COMPLIANCE_SCORE_LINE}
    </p>

    {#if node === ''}
        <div class="panel">
            <h2>Domains — area ∝ experiments × criticality, hue = band (click to drill)</h2>
            {#if tree.children.length === 0}
                <div class="state">No org structure on record — configure org.yaml.</div>
            {:else if treemapOption}
                <EChart
                    id="score-treemap"
                    option={treemapOption}
                    height={420}
                    onclick={onTreemapClick}
                    label="Scores of {tree.name} as a treemap"
                />
            {/if}
        </div>
    {:else}
        <div class="panel">
            <h2>
                {tree.name} — {tree.children.length} direct member{tree.children.length === 1
                    ? ''
                    : 's'}, weakest first
            </h2>
            {#if tree.children.length === 0}
                <div class="state">No members below this node.</div>
            {:else}
                <table class="data">
                    <thead>
                        <tr>
                            <th>Member</th><th>Score</th><th>Band</th><th>Δ</th><th>Sparkline</th>
                            <th>Coverage</th><th>Weakest member</th>
                        </tr>
                    </thead>
                    <tbody>
                        <tr class="self">
                            <td>{tree.name} <span class="kind">({tree.kind}, this node)</span></td>
                            <td class="mono">{fmtScore(tree.score)}</td>
                            <td
                                ><span class="bandg" style="color: {bandColor(tree.band)}"
                                    >{bandGlyph(tree.band)}</span
                                >
                                {bandLabel(tree.band)}</td
                            >
                            <td class="mono delta {deltaClass(tree.delta)}"
                                >{fmtDelta(tree.delta)}</td
                            >
                            <td
                                ><Sparkline
                                    points={sparkPoints}
                                    color={bandColor(tree.band)}
                                    width={110}
                                    height={26}
                                /></td
                            >
                            <td class="mono">{tree.scored}/{tree.expected}</td>
                            <td>{tree.weakest ?? '—'}</td>
                        </tr>
                        {#each tree.children as c (c.path)}
                            <tr class="clickable" onclick={() => drill(c.path)}>
                                <td
                                    ><span class="indent">└</span>
                                    {c.name} <span class="kind">({c.kind})</span></td
                                >
                                <td class="mono">{fmtScore(c.score)}</td>
                                <td
                                    ><span class="bandg" style="color: {bandColor(c.band)}"
                                        >{bandGlyph(c.band)}</span
                                    >
                                    {bandLabel(c.band)}</td
                                >
                                <td class="mono" style="color: var(--text-faint)">—</td>
                                <td style="color: var(--text-faint)">—</td>
                                <td class="mono">{c.scored}/{c.expected}</td>
                                <td>{c.weakest ?? '—'}</td>
                            </tr>
                        {/each}
                    </tbody>
                </table>
            {/if}
        </div>
    {/if}
{/if}

<style>
    .views {
        display: flex;
        margin-left: 12px;
    }
    .views button {
        background: var(--bg-raised);
        color: var(--text-dim);
        border: 1px solid var(--border-strong);
        padding: 3px 10px;
        font: inherit;
        font-size: 12px;
        cursor: pointer;
    }
    .views button:first-child {
        border-radius: 4px 0 0 4px;
    }
    .views button:last-child {
        border-radius: 0 4px 4px 0;
        border-left: none;
    }
    .views button.on {
        color: var(--text);
        background: var(--bg-hover);
        border-color: var(--accent);
    }
    .crumbs {
        display: flex;
        align-items: center;
        gap: 4px;
        margin-left: 12px;
    }
    .crumbs button {
        background: none;
        border: none;
        color: var(--accent);
        cursor: pointer;
        font-size: 13px;
        padding: 2px 4px;
    }
    .crumbs button.current {
        color: var(--text);
        font-weight: 600;
    }
    .crumbs .sep {
        color: var(--text-faint);
    }
    .controls {
        margin-left: auto;
    }
    .cards {
        display: grid;
        grid-template-columns: repeat(4, 1fr);
        gap: 12px;
        margin-bottom: 12px;
    }
    .card .label {
        color: var(--text-dim);
        font-size: 11.5px;
        text-transform: uppercase;
        letter-spacing: 0.6px;
        margin-bottom: 6px;
    }
    .card .value {
        font-size: 24px;
        font-weight: 600;
    }
    .card .value.weakest {
        font-size: 14px;
        font-weight: 500;
        word-break: break-word;
    }
    .card .glyph {
        margin-right: 6px;
        font-size: 18px;
    }
    .card .hint {
        color: var(--text-faint);
        font-size: 11.5px;
        margin-top: 4px;
    }
    .meaning {
        color: var(--text-dim);
        font-size: 12.5px;
        margin: 0 0 12px;
    }
    .kind {
        color: var(--text-faint);
        font-size: 11.5px;
    }
    .indent {
        color: var(--text-faint);
        margin-right: 4px;
    }
    tr.self {
        background: var(--bg-raised);
    }
    .bandg {
        font-size: 12px;
    }
</style>
