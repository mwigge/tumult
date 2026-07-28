<script lang="ts">
  import { page } from '$app/stores';
  import { goto } from '$app/navigation';
  import { api, fmtAgo, fmtDuration, shortId } from '$lib/api';
  import type { TraceDurations, TraceRow } from '$lib/types';
  import StatusBadge from '$lib/components/StatusBadge.svelte';
  import RangeSwitch from '$lib/components/RangeSwitch.svelte';
  import EChart from '$lib/components/EChart.svelte';
  import { CHART } from '$lib/echarts';
  import type { EChartsCoreOption } from '$lib/echarts';

  const filters = $derived({
    range: $page.url.searchParams.get('range') ?? '24h',
    service: $page.url.searchParams.get('service') ?? '',
    min_duration_ms: $page.url.searchParams.get('min_duration_ms') ?? '',
    outcome: $page.url.searchParams.get('outcome') ?? ''
  });

  let rows: TraceRow[] | null = $state(null);
  let durations: TraceDurations | null = $state(null);
  let maxDuration = $state(1);
  let error: string | null = $state(null);

  $effect(() => {
    // Touch every filter so the effect re-runs on any change; debounce for
    // the free-text inputs.
    const params = { ...filters };
    let cancelled = false;
    rows = null;
    error = null;
    const t = setTimeout(() => {
      api
        .traces(params)
        .then((r) => {
          if (cancelled) return;
          // Slowest first (sorted here: array callbacks inside $derived lose
          // their contextual types under svelte-check).
          rows = [...r.traces].sort((x, y) => y.duration_ns - x.duration_ns);
          maxDuration = rows[0]?.duration_ns ?? 1;
        })
        .catch((e) => !cancelled && (error = String(e)));
      api
        .traceDurations(params.range)
        .then((d) => !cancelled && (durations = d))
        .catch(() => {});
    }, 200);
    return () => {
      cancelled = true;
      clearTimeout(t);
    };
  });

  function setFilter(key: string, value: string) {
    const params = new URLSearchParams($page.url.searchParams);
    if (value) params.set(key, value);
    else params.delete(key);
    goto(`?${params}`, { replaceState: true, keepFocus: true, noScroll: true });
  }


  const scatterOption: EChartsCoreOption = $derived.by(() => {
    if (!durations || durations.points.length === 0) return {};
    const markLines = [
      { v: durations.p50_ms, label: 'p50', color: CHART.text },
      { v: durations.p95_ms, label: 'p95', color: CHART.warn },
      { v: durations.p99_ms, label: 'p99', color: CHART.fail }
    ].filter((l) => l.v !== null);
    const data: [number, number, string][] = durations.points.map((p) => [
      p.ts_ns / 1e6,
      p.duration_ms,
      p.trace_id
    ]);
    return {
      grid: { left: 64, right: 24, top: 20, bottom: 24 },
      tooltip: {
        ...CHART.tooltip,
        formatter: (p: { data?: [number, number, string] }) => {
          const d = p.data;
          if (!d) return '';
          return `${d[2]}<br/>${new Date(d[0]).toLocaleString()}<br/><b>${d[1].toFixed(0)}ms</b>`;
        }
      },
      xAxis: {
        type: 'time',
        axisLine: { lineStyle: { color: CHART.axis } },
        axisLabel: { color: CHART.text }
      },
      yAxis: {
        type: 'log',
        name: 'duration ms',
        nameTextStyle: { color: CHART.text },
        axisLabel: { color: CHART.text },
        splitLine: { lineStyle: { color: CHART.split } }
      },
      series: [
        {
          type: 'scatter',
          symbolSize: 9,
          itemStyle: { color: CHART.accent },
          data,
          markLine: {
            silent: true,
            symbol: 'none',
            label: { color: CHART.text, formatter: '{b}' },
            lineStyle: { type: 'dashed' },
            data: markLines.map((l) => ({
              name: l.label,
              yAxis: l.v,
              lineStyle: { color: l.color, type: 'dashed' }
            }))
          }
        }
      ]
    };
  });

  function onPoint(params: { data?: unknown }) {
    const d = params.data as [number, number, string] | undefined;
    if (d?.[2]) goto(`/traces/${encodeURIComponent(d[2])}`);
  }
</script>

<div class="page-head">
  <h1>Traces</h1>
  <span class="sub">{rows ? `${rows.length} trace${rows.length === 1 ? '' : 's'}` : 'slowest first'}</span>
  <div class="controls">
    <select value={filters.outcome} onchange={(e) => setFilter('outcome', e.currentTarget.value)}>
      <option value="">all outcomes</option>
      <option value="completed">Completed</option>
      <option value="deviated">Deviated</option>
      <option value="failed">Failed</option>
      <option value="incomplete">incomplete</option>
    </select>
    <input
      type="search"
      placeholder="service…"
      value={filters.service}
      oninput={(e) => setFilter('service', e.currentTarget.value)}
    />
    <input
      type="number"
      min="0"
      placeholder="min ms"
      style="width: 90px"
      value={filters.min_duration_ms}
      oninput={(e) => setFilter('min_duration_ms', e.currentTarget.value)}
    />
    <RangeSwitch value={filters.range} onchange={(r) => setFilter('range', r)} />
  </div>
</div>

<div class="panel">
  {#if durations && durations.points.length > 0}
    <EChart option={scatterOption} height={220} onclick={onPoint} />
  {:else if rows}
    <div class="state">No root-span durations in this window.</div>
  {:else}
    <div class="skeleton" style="height: 220px"></div>
  {/if}
</div>

<div class="panel">
  {#if error}
    <div class="state error">Failed to load traces: {error}</div>
  {:else if !rows}
    <div class="skeleton" style="height: 240px"></div>
  {:else if rows.length === 0}
    <div class="state">No traces match these filters.</div>
  {:else}
    <table class="data">
      <thead>
        <tr>
          <th>Root span</th><th>Service</th><th>Started</th>
          <th>Spans</th><th>Errors</th><th style="width: 30%">Duration</th><th>Status</th>
        </tr>
      </thead>
      <tbody>
        {#each rows as row (row.trace_id)}
          <tr class="clickable" onclick={() => goto(`/traces/${encodeURIComponent(row.trace_id)}`)}>
            <td class="mono" title={row.trace_id}>{row.root_name ?? shortId(row.trace_id)}</td>
            <td class="mono">{row.service_name ?? '—'}</td>
            <td title={new Date(row.started_ns / 1e6).toISOString()}>{fmtAgo(row.started_ns)}</td>
            <td class="mono">{row.span_count}</td>
            <td class="mono" style="color: {row.error_count > 0 ? 'var(--fail)' : 'inherit'}">
              {row.error_count}
            </td>
            <td>
              <div class="dur">
                <div class="bar" style="width: {Math.max(2, (row.duration_ns / maxDuration) * 100)}%"></div>
                <span class="mono">{fmtDuration(row.duration_ns)}</span>
              </div>
            </td>
            <td>
              {#if row.status}
                <StatusBadge status={row.status} />
              {:else}
                <span class="sub">—</span>
              {/if}
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
</div>

<style>
  .controls {
    margin-left: auto;
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }
  .dur {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .bar {
    height: 6px;
    border-radius: 3px;
    background: var(--accent-dim);
    flex: 0 1 auto;
    min-width: 2px;
  }
  .dur span {
    margin-left: auto;
    white-space: nowrap;
  }
</style>
