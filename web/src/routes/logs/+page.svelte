<script lang="ts">
  import { page } from '$app/stores';
  import { goto } from '$app/navigation';
  import { api, fmtAgo, fmtTs } from '$lib/api';
  import type { LogEntry, LogVolume } from '$lib/types';
  import RangeSwitch from '$lib/components/RangeSwitch.svelte';
  import EChart from '$lib/components/EChart.svelte';
  import { CHART } from '$lib/echarts';
  import type { EChartsCoreOption } from '$lib/echarts';

  const SEVERITIES = ['ERROR', 'WARN', 'INFO', 'DEBUG', 'TRACE'];
  const SEV_COLORS: Record<string, string> = {
    ERROR: CHART.fail,
    FATAL: CHART.fail,
    WARN: CHART.warn,
    WARNING: CHART.warn,
    INFO: CHART.ok,
    DEBUG: CHART.text,
    TRACE: CHART.text
  };

  const filters = $derived({
    range: $page.url.searchParams.get('range') ?? '24h',
    severity: $page.url.searchParams.get('severity') ?? '',
    service: $page.url.searchParams.get('service') ?? '',
    q: $page.url.searchParams.get('q') ?? ''
  });

  let rows: LogEntry[] | null = $state(null);
  let volume: LogVolume | null = $state(null);
  let error: string | null = $state(null);
  let expanded: number | null = $state(null);

  $effect(() => {
    // Touch every filter so the effect re-runs on any change; debounce for
    // the free-text inputs.
    const params = { ...filters };
    let cancelled = false;
    rows = null;
    error = null;
    const t = setTimeout(() => {
      api
        .logs(params)
        .then((r) => !cancelled && (rows = r.logs))
        .catch((e) => !cancelled && (error = String(e)));
      api
        .logsVolume({ ...params, interval: params.range === '24h' ? '1h' : '1d' })
        .then((v) => !cancelled && (volume = v))
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

  function sevClass(sev: string | null): 'ok' | 'warn' | 'fail' | 'neutral' {
    switch ((sev ?? '').toUpperCase()) {
      case 'ERROR':
      case 'FATAL':
        return 'fail';
      case 'WARN':
      case 'WARNING':
        return 'warn';
      case 'INFO':
        return 'ok';
      default:
        return 'neutral';
    }
  }

  // Pivot the (bucket, severity) rows into one stacked bar series per
  // severity, aligned across the full bucket set so gaps render as zero.
  const volumeOption: EChartsCoreOption = $derived.by(() => {
    if (!volume || volume.rows.length === 0) return {};
    const buckets = [...new Set(volume.rows.map((r) => r.ts))].sort((a, b) => a - b);
    const ord = (s: string) => {
      const i = SEVERITIES.indexOf(s.toUpperCase());
      return i === -1 ? SEVERITIES.length : i;
    };
    const sevs = [...new Set(volume.rows.map((r) => r.severity))].sort(
      (a, b) => ord(a) - ord(b)
    );
    const byKey = new Map(volume.rows.map((r) => [`${r.ts}|${r.severity}`, r.count]));
    return {
      grid: { left: 48, right: 16, top: 28, bottom: 24 },
      tooltip: { trigger: 'axis', ...CHART.tooltip },
      legend: { textStyle: { color: CHART.text }, top: 0 },
      xAxis: {
        type: 'time',
        axisLine: { lineStyle: { color: CHART.axis } },
        axisLabel: { color: CHART.text }
      },
      yAxis: {
        type: 'value',
        minInterval: 1,
        axisLabel: { color: CHART.text },
        splitLine: { lineStyle: { color: CHART.split } }
      },
      series: sevs.map((sev) => ({
        name: sev,
        type: 'bar',
        stack: 'volume',
        barMaxWidth: 40,
        itemStyle: { color: SEV_COLORS[sev.toUpperCase()] ?? CHART.accent },
        data: buckets.map((b) => [b * 1000, byKey.get(`${b}|${sev}`) ?? 0])
      }))
    };
  });
</script>

<div class="page-head">
  <h1>Logs</h1>
  <span class="sub">{rows ? `${rows.length} row${rows.length === 1 ? '' : 's'}` : 'newest first'}</span>
  <div class="controls">
    <input
      type="search"
      placeholder="search body…"
      value={filters.q}
      oninput={(e) => setFilter('q', e.currentTarget.value)}
    />
    <select value={filters.severity} onchange={(e) => setFilter('severity', e.currentTarget.value)}>
      <option value="">all severities</option>
      {#each SEVERITIES as s (s)}
        <option value={s}>{s}</option>
      {/each}
    </select>
    <input
      type="search"
      placeholder="service…"
      value={filters.service}
      oninput={(e) => setFilter('service', e.currentTarget.value)}
    />
    <RangeSwitch value={filters.range} onchange={(r) => setFilter('range', r)} />
  </div>
</div>

<div class="panel">
  {#if volume && volume.rows.length > 0}
    <EChart option={volumeOption} height={180} />
  {:else if rows}
    <div class="state">No log volume in this window.</div>
  {:else}
    <div class="skeleton" style="height: 180px"></div>
  {/if}
</div>

<div class="panel">
  {#if error}
    <div class="state error">Failed to load logs: {error}</div>
  {:else if !rows}
    <div class="skeleton" style="height: 240px"></div>
  {:else if rows.length === 0}
    <div class="state">No logs match these filters.</div>
  {:else}
    <table class="data">
      <thead>
        <tr>
          <th>Time</th><th>Severity</th><th>Service</th><th>Body</th>
        </tr>
      </thead>
      <tbody>
        {#each rows as row, i (row.ts_ns + row.body)}
          <tr
            class="clickable"
            class:expanded={expanded === i}
            onclick={() => (expanded = expanded === i ? null : i)}
          >
            <td class="mono" title={fmtTs(row.ts_ns)}>{fmtAgo(row.ts_ns)}</td>
            <td><span class="badge {sevClass(row.severity_text)}">{row.severity_text ?? '—'}</span></td>
            <td class="mono">{row.service_name ?? '—'}</td>
            <td class="body-cell">{row.body}</td>
          </tr>
          {#if expanded === i}
            <tr class="detail-row">
              <td colspan="4">
                <div class="detail">
                  <div class="links">
                    {#if row.experiment_id}
                      <a href="/experiments/{row.experiment_id}">experiment {row.experiment_id}</a>
                    {/if}
                    {#if row.trace_id}
                      <a href="/traces/{row.trace_id}">trace {row.trace_id}</a>
                    {/if}
                    {#if !row.experiment_id && !row.trace_id}
                      <span class="sub">no experiment or trace linkage</span>
                    {/if}
                  </div>
                  {#if Object.keys(row.log_attrs).length > 0}
                    <table class="attrs">
                      <tbody>
                        {#each Object.entries(row.log_attrs) as [k, v] (k)}
                          <tr><td class="mono key">{k}</td><td class="mono">{v}</td></tr>
                        {/each}
                      </tbody>
                    </table>
                  {/if}
                </div>
              </td>
            </tr>
          {/if}
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
  .body-cell {
    max-width: 640px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  tr.expanded td {
    border-bottom: none;
  }
  .detail-row td {
    padding-top: 0;
  }
  .detail {
    padding: 4px 0 8px;
  }
  .links {
    display: flex;
    gap: 16px;
    margin-bottom: 8px;
    font-size: 13px;
  }
  table.attrs td {
    padding: 2px 8px 2px 0;
    border: none;
    font-size: 12px;
  }
  table.attrs .key {
    color: var(--text-dim);
  }
</style>
