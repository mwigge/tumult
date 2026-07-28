<script lang="ts">
  // Generic ECharts host: init once, re-set options on change, resize with
  // the container, dispose on unmount.
  import echarts from '$lib/echarts';
  import type { EChartsCoreOption } from '$lib/echarts';
  import { onMount } from 'svelte';

  let { option, height = 260 }: { option: EChartsCoreOption; height?: number } = $props();

  let el: HTMLDivElement;
  let chart: ReturnType<typeof echarts.init> | null = null;

  onMount(() => {
    chart = echarts.init(el);
    chart.setOption(option);
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
