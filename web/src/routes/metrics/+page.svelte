<script lang="ts">
  import { page } from '$app/stores';
  import { goto } from '$app/navigation';
  import { onMount } from 'svelte';
  import { api } from '$lib/api';
  import type { MetricCatalogEntry, MetricQueryResult } from '$lib/types';
  import RangeSwitch from '$lib/components/RangeSwitch.svelte';
  import EChart from '$lib/components/EChart.svelte';
  import { CHART } from '$lib/echarts';
  import type { EChartsCoreOption } from '$lib/echarts';

  const INTERVALS = ['5m', '1h', '1d'];
  const CHART_TYPES = ['line', 'area', 'bar'] as const;
  const PALETTE = [CHART.accent, CHART.ok, CHART.warn, CHART.fail, '#a78bfa', '#f472b6'];

  const filters = $derived({
    name: $page.url.searchParams.get('name') ?? '',
    group_by: $page.url.searchParams.get('group_by') ?? '',
    chart: $page.url.searchParams.get('chart') ?? 'line',
    interval: $page.url.searchParams.get('interval') ?? '1h',
    range: $page.url.searchParams.get('range') ?? '24h'
  });

  let catalog: MetricCatalogEntry[] | null = $state(null);
  let selected: MetricCatalogEntry | null = $state(null);
  let result: MetricQueryResult | null = $state(null);
  let error: string | null = $state(null);
  // Series pre-built for the chart at fetch time (array callbacks inside
  // $derived lose their contextual types under svelte-check).
  let series: { name: string; data: [number, number][] }[] = $state([]);

  $effect(() => {
    const name = filters.name;
    const list = catalog;
    selected = (name && list?.find((m) => m.name === name)) || null;
  });

  onMount(() => {
    api
      .metricsCatalog()
      .then((c) => {
        catalog = c.metrics;
        // Default to the first catalog metric when the URL names none.
        if (!filters.name && c.metrics.length > 0) setFilter('name', c.metrics[0].name);
      })
      .catch((e) => (error = String(e)));
  });

  $effect(() => {
    const params = { ...filters };
    if (!params.name) return;
    let cancelled = false;
    result = null;
    error = null;
    api
      .metricQuery(params)
      .then((r) => {
        if (cancelled) return;
        result = r;
        series = r.series.flatMap((s) => {
          const label = s.group ?? (r.group_by ? '(none)' : r.name);
          const out: { name: string; data: [number, number][] }[] = [];
          if (r.type === 'histogram') {
            out.push({
              name: `${label} avg`,
              data: s.points
                .filter((p) => p.avg != null)
                .map((p) => [p.ts * 1000, p.avg as number])
            });
            out.push({
              name: `${label} p95`,
              data: s.points
                .filter((p) => p.p95 != null)
                .map((p) => [p.ts * 1000, p.p95 as number])
            });
          } else {
            out.push({
              name: label,
              data: s.points
                .filter((p) => p.v != null)
                .map((p) => [p.ts * 1000, p.v as number])
            });
          }
          return out;
        });
      })
      .catch((e) => !cancelled && (error = String(e)));
    return () => {
      cancelled = true;
    };
  });

  function setFilter(key: string, value: string) {
    const params = new URLSearchParams($page.url.searchParams);
    if (value) params.set(key, value);
    else params.delete(key);
    goto(`?${params}`, { replaceState: true, keepFocus: true, noScroll: true });
  }

  const chartOption: EChartsCoreOption = $derived.by(() => {
    if (series.length === 0) return {};
    const stacked = filters.chart === 'area' && series.length > 1;
    return {
      grid: { left: 56, right: 20, top: 32, bottom: 24 },
      tooltip: { trigger: 'axis', ...CHART.tooltip },
      legend: { textStyle: { color: CHART.text }, top: 0 },
      xAxis: {
        type: 'time',
        axisLine: { lineStyle: { color: CHART.axis } },
        axisLabel: { color: CHART.text }
      },
      yAxis: {
        type: 'value',
        axisLabel: { color: CHART.text },
        splitLine: { lineStyle: { color: CHART.split } }
      },
      series: series.map((s, i) => ({
        name: s.name,
        type: filters.chart === 'bar' ? 'bar' : 'line',
        stack: stacked ? 'total' : undefined,
        areaStyle: filters.chart === 'area' ? {} : undefined,
        showSymbol: false,
        itemStyle: { color: PALETTE[i % PALETTE.length] },
        data: s.data
      }))
    };
  });
</script>

<div class="page-head">
  <h1>Metrics</h1>
  {#if selected}
    <span class="badge neutral">{selected.types[0]}</span>
  {/if}
  <div class="controls">
    <select value={filters.name} onchange={(e) => setFilter('name', e.currentTarget.value)}>
      {#if !catalog}
        <option value="">loading…</option>
      {:else}
        {#each catalog as m (m.name)}
          <option value={m.name}>{m.name}</option>
        {/each}
      {/if}
    </select>
    <select
      value={filters.group_by}
      onchange={(e) => setFilter('group_by', e.currentTarget.value)}
      disabled={!selected || selected.dimensions.length === 0}
      title={selected && selected.dimensions.length === 0 ? 'no attributes on this metric' : ''}
    >
      <option value="">no grouping</option>
      {#each selected?.dimensions ?? [] as d (d)}
        <option value={d}>group by {d}</option>
      {/each}
    </select>
    <div class="seg" role="group" aria-label="chart type">
      {#each CHART_TYPES as t (t)}
        <button class:active={filters.chart === t} onclick={() => setFilter('chart', t)}>{t}</button>
      {/each}
    </div>
    <select value={filters.interval} onchange={(e) => setFilter('interval', e.currentTarget.value)}>
      {#each INTERVALS as i (i)}
        <option value={i}>{i}</option>
      {/each}
    </select>
    <RangeSwitch value={filters.range} onchange={(r) => setFilter('range', r)} />
  </div>
</div>

<div class="panel">
  {#if error}
    <div class="state error">Failed to load metric: {error}</div>
  {:else if catalog === null || (filters.name && result === null)}
    <div class="skeleton" style="height: 320px"></div>
  {:else if catalog.length === 0}
    <div class="state">No raw metrics stored yet.</div>
  {:else if series.length === 0}
    <div class="state">No points for this metric in the selected window.</div>
  {:else}
    <EChart option={chartOption} height={360} />
  {/if}
</div>

{#if result && result.type === 'histogram'}
  <p class="sub" style="margin-top: 8px">
    histogram points show the bucketed average and an interpolated p95 (approximate within
    buckets, clamped at the last explicit bound).
  </p>
{/if}

<style>
  .controls {
    margin-left: auto;
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
    align-items: center;
  }
</style>
