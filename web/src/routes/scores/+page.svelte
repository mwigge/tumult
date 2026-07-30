<script lang="ts">
  // Org hierarchy rollups: root = KPI cards + treemap; drill by click with
  // breadcrumb + ?node= URL state; non-root = indented tree table.
  import { page } from '$app/stores';
  import { goto } from '$app/navigation';
  import { api } from '$lib/api';
  import type { EChartsCoreOption } from '$lib/echarts';
  import { CHART } from '$lib/echarts';
  import type { OrgNodeScore, ScoreTree, SparkPoint } from '$lib/types';
  import EChart from '$lib/components/EChart.svelte';
  import Sparkline from '$lib/components/Sparkline.svelte';
  import RangeSwitch from '$lib/components/RangeSwitch.svelte';

  const node = $derived($page.url.searchParams.get('node') ?? '');
  const range = $derived($page.url.searchParams.get('range') ?? '7d');

  let tree: ScoreTree | null = $state(null);
  let error: string | null = $state(null);

  $effect(() => {
    const n = node;
    const r = range;
    let cancelled = false;
    tree = null;
    error = null;
    api
      .scoreTree(n, r)
      .then((t) => !cancelled && (tree = t))
      .catch((e) => !cancelled && (error = String(e)));
    return () => {
      cancelled = true;
    };
  });

  function drill(path: string) {
    const params = new URLSearchParams($page.url.searchParams);
    if (path) params.set('node', path);
    else params.delete('node');
    goto(`?${params}`, { keepFocus: true, noScroll: true });
  }

  function setRange(r: string) {
    const params = new URLSearchParams($page.url.searchParams);
    params.set('range', r);
    goto(`?${params}`, { replaceState: true, keepFocus: true, noScroll: true });
  }

  // Band hues from the Okabe-Ito palette (colour-blind safe).
  const BAND_COLOR: Record<string, string> = {
    good: '#009E73',
    fair: '#E69F00',
    poor: '#D55E00'
  };
  const BAND_GLYPH: Record<string, string> = { good: '●', fair: '▲', poor: '×' };
  const bandColor = (b: string) => BAND_COLOR[b] ?? '#5b6772';
  const bandGlyph = (b: string) => BAND_GLYPH[b] ?? '○';

  // Breadcrumb segments for the current node path.
  const crumbs = $derived(node === '' ? [] : node.split('/'));

  let treemapOption: EChartsCoreOption | null = $state(null);
  $effect(() => {
    if (!tree || node !== '') {
      treemapOption = null;
      return;
    }
    const children: OrgNodeScore[] = tree.children;
    treemapOption = {
      tooltip: {
        ...CHART.tooltip,
        formatter: (p: { data?: { path?: string; score?: number; band?: string; scored?: number; expected?: number } }) => {
          const d = p.data ?? {};
          return `<b>${d.path ?? ''}</b><br/>score ${Number(d.score ?? 0).toFixed(1)} (${d.band ?? ''})` +
            `<br/>coverage ${d.scored ?? 0}/${d.expected ?? 0}`;
        }
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
            formatter: (p: { name?: string; data?: { score?: number } }) =>
              `${p.name ?? ''}\n${Number(p.data?.score ?? 0).toFixed(1)}`,
            color: '#0b0e11',
            fontWeight: 600,
            fontSize: 12
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
            itemStyle: { color: bandColor(c.band) }
          }))
        }
      ]
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
  const fmtScore = (v: number) => v.toFixed(1);
  const fmtDelta = (d: number) => `${d >= 0 ? '+' : ''}${d.toFixed(1)}`;
  const deltaCls = (d: number) => (d > 0.5 ? 'up' : d < -0.5 ? 'down' : 'flat');
</script>

<div class="page-head">
  <h1>Scores</h1>
  <nav class="crumbs" aria-label="org breadcrumb">
    <button class:current={node === ''} onclick={() => drill('')}>company</button>
    {#each crumbs as seg, i (i)}
      <span class="sep">/</span>
      <button class:current={i === crumbs.length - 1} onclick={() => drill(crumbs.slice(0, i + 1).join('/'))}>{seg}</button>
    {/each}
  </nav>
  <div class="controls">
    <RangeSwitch value={range} onchange={setRange} />
  </div>
</div>

{#if error}
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
      <div class="hint">{tree.band}</div>
    </div>
    <div class="panel card">
      <div class="label">Δ vs previous {range}</div>
      <div class="value mono delta {deltaCls(tree.delta)}">{fmtDelta(tree.delta)}</div>
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

  {#if node === ''}
    <div class="panel">
      <h2>Domains — area ∝ experiments × criticality, hue = band (click to drill)</h2>
      {#if tree.children.length === 0}
        <div class="state">No org structure on record — configure org.yaml.</div>
      {:else if treemapOption}
        <EChart option={treemapOption} height={420} onclick={onTreemapClick} />
      {/if}
    </div>
  {:else}
    <div class="panel">
      <h2>{tree.name} — {tree.children.length} direct member{tree.children.length === 1 ? '' : 's'}, weakest first</h2>
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
              <td><span class="bandg" style="color: {bandColor(tree.band)}">{bandGlyph(tree.band)}</span> {tree.band}</td>
              <td class="mono delta {deltaCls(tree.delta)}">{fmtDelta(tree.delta)}</td>
              <td><Sparkline points={sparkPoints} color={bandColor(tree.band)} width={110} height={26} /></td>
              <td class="mono">{tree.scored}/{tree.expected}</td>
              <td>{tree.weakest ?? '—'}</td>
            </tr>
            {#each tree.children as c (c.path)}
              <tr class="clickable" onclick={() => drill(c.path)}>
                <td><span class="indent">└</span> {c.name} <span class="kind">({c.kind})</span></td>
                <td class="mono">{fmtScore(c.score)}</td>
                <td><span class="bandg" style="color: {bandColor(c.band)}">{bandGlyph(c.band)}</span> {c.band}</td>
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
