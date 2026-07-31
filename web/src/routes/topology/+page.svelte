<script lang="ts">
  import { page } from '$app/stores';
  import { goto } from '$app/navigation';
  import { api, fmtDuration } from '$lib/api';
  import type { Topology, TopologyNode } from '$lib/types';
  import RangeSwitch from '$lib/components/RangeSwitch.svelte';
  import EChart from '$lib/components/EChart.svelte';
  import { CHART } from '$lib/echarts';
  import type { EChartsCoreOption } from '$lib/echarts';

  const filters = $derived({
    range: $page.url.searchParams.get('range') ?? '24h'
  });

  let topo: Topology | null = $state(null);
  let error: string | null = $state(null);
  // Graph data pre-built at fetch time (array callbacks inside $derived
  // lose their contextual types under svelte-check).
  let graphNodes: {
    id: string;
    name: string;
    symbolSize: number;
    itemStyle: { color: string };
    label: { show: boolean };
    raw: TopologyNode;
  }[] = $state([]);
  let graphEdges: { source: string; target: string; value: number }[] = $state([]);

  // Error rate → green/amber/red.
  function healthColor(node: TopologyNode): string {
    if (node.runs <= 0 || node.errors <= 0) return CHART.ok;
    const rate = node.errors / node.runs;
    if (rate < 0.2) return CHART.warn;
    return CHART.fail;
  }

  $effect(() => {
    const params = { ...filters };
    let cancelled = false;
    topo = null;
    error = null;
    api
      .topology(params.range)
      .then((t) => {
        if (cancelled) return;
        topo = t;
        graphNodes = t.nodes.map((n) => ({
          id: n.id,
          name: n.name,
          symbolSize: Math.max(14, Math.sqrt(n.runs) * 8),
          itemStyle: {
            color: n.type === 'target' ? CHART.accent : healthColor(n)
          },
          label: { show: true },
          raw: n
        }));
        graphEdges = t.edges.map((e) => ({
          source: e.from_id,
          target: e.to_id,
          value: e.weight
        }));
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

  const graphOption: EChartsCoreOption = $derived.by(() => {
    if (graphNodes.length === 0) return {};
    return {
      tooltip: {
        ...CHART.tooltip,
        formatter: (p: { dataType?: string; data?: { raw?: TopologyNode; value?: number } }) => {
          if (p.dataType === 'edge') return `${p.data?.value ?? 0} calls`;
          const n = p.data?.raw;
          if (!n) return '';
          const rate = n.runs > 0 ? ((n.errors / n.runs) * 100).toFixed(1) : '0.0';
          return `<b>${n.name}</b> (${n.type})<br/>${n.runs} spans · ${n.errors} errors (${rate}%)<br/>avg ${fmtDuration(n.avg_duration_ns)}`;
        }
      },
      series: [
        {
          type: 'graph',
          layout: 'force',
          roam: true,
          draggable: true,
          force: { repulsion: 320, edgeLength: [60, 160] },
          label: { color: '#d8dee4', fontSize: 11, position: 'bottom' },
          edgeSymbol: ['none', 'arrow'],
          edgeSymbolSize: 6,
          lineStyle: { color: CHART.axis, width: 1.5, curveness: 0.1 },
          emphasis: { focus: 'adjacency' },
          data: graphNodes,
          links: graphEdges
        }
      ]
    };
  });

  function onNode(params: { data?: unknown }) {
    const d = params.data as { raw?: TopologyNode } | undefined;
    // Services drill into the traces explorer; targets have no matching
    // filter yet, so they stay a no-op.
    if (d?.raw?.type === 'service') {
      goto(`/traces?service=${encodeURIComponent(d.raw.name)}&range=${filters.range}`);
    }
  }
</script>

<div class="page-head">
  <h1>Topology</h1>
  <span class="sub">
    {topo ? `${topo.nodes.length} nodes · ${topo.edges.length} edges` : 'services and targets'}
  </span>
  <div class="controls">
    <RangeSwitch value={filters.range} onchange={(r) => setFilter('range', r)} />
  </div>
</div>

<div class="panel">
  {#if error}
    <div class="state error">Failed to load topology: {error}</div>
  {:else if !topo}
    <div class="skeleton" style="height: 420px"></div>
  {:else if topo.nodes.length === 0}
    <div class="state">No spans in this window.</div>
  {:else}
    <EChart option={graphOption} height={480} onclick={onNode} />
    <p class="sub" style="margin-top: 8px">
      node size = span count · services colored by error rate, targets in blue · click a service
      to open its traces
    </p>
  {/if}
</div>

<style>
  .controls {
    margin-left: auto;
    display: flex;
    gap: 8px;
  }
</style>
