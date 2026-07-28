<script lang="ts">
  import { page } from '$app/stores';
  import { goto } from '$app/navigation';
  import { api } from '$lib/api';
  import type { Overview } from '$lib/types';
  import { CHART } from '$lib/echarts';
  import KpiCard from '$lib/components/KpiCard.svelte';
  import RangeSwitch from '$lib/components/RangeSwitch.svelte';
  import EChart from '$lib/components/EChart.svelte';

  const range = $derived($page.url.searchParams.get('range') ?? '24h');

  let data: Overview | null = $state(null);
  let error: string | null = $state(null);

  $effect(() => {
    let cancelled = false;
    data = null;
    error = null;
    api
      .overview(range)
      .then((d) => !cancelled && (data = d))
      .catch((e) => !cancelled && (error = String(e)));
    return () => {
      cancelled = true;
    };
  });

  function setRange(r: string) {
    goto(`?range=${r}`, { replaceState: true, keepFocus: true, noScroll: true });
  }

  const dayStr = (ts: number) => new Date(ts * 1000).toISOString().slice(0, 10);

  const perDayOption = $derived.by(() => {
    if (!data) return {};
    const points = data.experiments_per_day;
    if (range === '24h') {
      return {
        ...tooltip(),
        grid: { left: 40, right: 12, top: 18, bottom: 26 },
        xAxis: axisCat(points.map((p) => dayStr(p.ts))),
        yAxis: axisVal(),
        series: [
          {
            type: 'bar',
            data: points.map((p) => p.v),
            itemStyle: { color: CHART.accent, borderRadius: [3, 3, 0, 0] },
            barMaxWidth: 44
          }
        ]
      };
    }
    const from = dayStr(data.from_ns / 1_000_000_000);
    const to = dayStr(data.to_ns / 1_000_000_000);
    const max = Math.max(1, ...points.map((p) => p.v));
    return {
      tooltip: { ...CHART.tooltip },
      visualMap: {
        min: 0,
        max,
        show: false,
        inRange: { color: ['#1c242c', '#2d5d84', CHART.accent] }
      },
      calendar: {
        range: [from, to],
        top: 34,
        left: 44,
        right: 12,
        cellSize: ['auto', 16],
        splitLine: { lineStyle: { color: CHART.axis } },
        itemStyle: { color: 'transparent', borderColor: CHART.axis },
        dayLabel: { color: CHART.text },
        monthLabel: { color: CHART.text },
        yearLabel: { show: false }
      },
      series: [
        {
          type: 'heatmap',
          coordinateSystem: 'calendar',
          data: points.map((p) => [dayStr(p.ts), p.v])
        }
      ]
    };
  });

  const faultsOption = $derived.by(() => {
    if (!data) return {};
    return {
      tooltip: { ...CHART.tooltip, trigger: 'item' },
      legend: { bottom: 0, textStyle: { color: CHART.text } },
      series: [
        {
          type: 'pie',
          radius: ['48%', '72%'],
          center: ['50%', '44%'],
          itemStyle: { borderColor: '#151b21', borderWidth: 2 },
          label: { show: false },
          data: data.faults.map((f) => ({
            name: f.fault_subtype ? `${f.fault_type} / ${f.fault_subtype}` : f.fault_type,
            value: f.count
          })),
          color: [CHART.warn, CHART.accent, CHART.fail, CHART.ok]
        }
      ]
    };
  });

  function tooltip() {
    return { tooltip: { ...CHART.tooltip } };
  }
  function axisCat(data: string[]) {
    return {
      type: 'category',
      data,
      axisLine: { lineStyle: { color: CHART.axis } },
      axisLabel: { color: CHART.text },
      axisTick: { show: false }
    };
  }
  function axisVal() {
    return {
      type: 'value',
      minInterval: 1,
      splitLine: { lineStyle: { color: CHART.split } },
      axisLabel: { color: CHART.text }
    };
  }
</script>

<div class="page-head">
  <h1>Overview</h1>
  <span class="sub">resilience posture for the selected window</span>
  <div style="margin-left: auto">
    <RangeSwitch value={range} onchange={setRange} />
  </div>
</div>

{#if error}
  <div class="state error panel">Failed to load overview: {error}</div>
{:else if !data}
  <div class="grid" style="grid-template-columns: repeat(5, 1fr); margin-bottom: 14px">
    {#each Array(5) as _, i (i)}
      <div class="skeleton" style="height: 92px"></div>
    {/each}
  </div>
  <div class="skeleton" style="height: 260px"></div>
{:else}
  <div class="grid kpis">
    {#each data.kpis as kpi (kpi.name)}
      <KpiCard {kpi} />
    {/each}
  </div>

  <div class="grid cols-2" style="margin-top: 14px">
    <div class="panel">
      <h2>Experiments per day</h2>
      {#if data.experiments_per_day.length === 0}
        <div class="state">No experiments in this window.</div>
      {:else}
        <EChart option={perDayOption} height={range === '24h' ? 220 : 200} />
      {/if}
    </div>
    <div class="panel">
      <h2>Fault breakdown</h2>
      {#if data.faults.length === 0}
        <div class="state">No fault-injection spans recorded in this window.</div>
      {:else}
        <EChart option={faultsOption} height={220} />
      {/if}
    </div>
  </div>

  <div class="panel" style="margin-top: 14px">
    <h2>Target systems</h2>
    {#if data.targets.length === 0}
      <div class="state">
        No target-system metadata in this window — tumult runs don't tag target
        systems yet, so this fills in once targets are annotated.
      </div>
    {:else}
      {@const max = Math.max(...data.targets.map((t) => t.experiments))}
      <table class="data">
        <thead>
          <tr><th>Target</th><th style="width: 40%">Experiments</th><th>n</th><th>Pass rate</th></tr>
        </thead>
        <tbody>
          {#each data.targets as t (t.target)}
            <tr>
              <td class="mono">{t.target}</td>
              <td>
                <div class="tbar" style="width: {(t.experiments / max) * 100}%"></div>
              </td>
              <td class="mono">{t.experiments}</td>
              <td class="mono">
                {t.pass_rate === null ? '—' : `${(t.pass_rate * 100).toFixed(0)}%`}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}
  </div>
{/if}

<style>
  .kpis {
    grid-template-columns: repeat(5, 1fr);
  }
  @media (max-width: 1200px) {
    .kpis {
      grid-template-columns: repeat(3, 1fr);
    }
  }
  @media (max-width: 800px) {
    .kpis {
      grid-template-columns: repeat(2, 1fr);
    }
  }
  .tbar {
    height: 8px;
    border-radius: 2px;
    background: var(--accent-dim);
    min-width: 2px;
  }
</style>
