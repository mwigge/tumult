<script lang="ts">
  // Generic ECharts host: init once, re-set options on change, resize with
  // the container, dispose on unmount.
  import echarts from '$lib/echarts';
  import type { EChartsCoreOption } from '$lib/echarts';
  import { onMount } from 'svelte';

  let {
    option,
    height = 260,
    onclick
  }: {
    option: EChartsCoreOption;
    height?: number;
    // ECharts 'click' event params (seriesType-specific shape) — used by the
    // traces scatter to navigate on point click.
    onclick?: (params: { data?: unknown }) => void;
  } = $props();

  let el: HTMLDivElement;
  let chart: ReturnType<typeof echarts.init> | null = null;

  onMount(() => {
    chart = echarts.init(el);
    chart.setOption(option);
    if (onclick) chart.on('click', onclick);
    const ro = new ResizeObserver(() => chart?.resize());
    ro.observe(el);
    return () => {
      ro.disconnect();
      chart?.dispose();
      chart = null;
    };
  });

  $effect(() => {
    if (chart && option) chart.setOption(option, { notMerge: true });
  });
</script>

<div bind:this={el} style="height: {height}px; width: 100%;"></div>
