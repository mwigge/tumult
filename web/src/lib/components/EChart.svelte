<script lang="ts">
    // Generic ECharts host: init once with the theme in force, re-set
    // options on change, re-init when the theme flips, resize with the
    // container, dispose on unmount. With an `id`, the option in force is
    // mirrored to `window.__tumultCharts[id]` so Playwright can assert on
    // the serialised option instead of pixels (charts render to canvas).
    import echarts, { chartTheme } from '#lib/echarts.js';
    import type { EChartsCoreOption } from '#lib/echarts.js';
    import { onMount } from 'svelte';

    let {
        option,
        height = 260,
        onclick,
        id,
    }: {
        option: EChartsCoreOption;
        height?: number;
        // ECharts 'click' event params (seriesType-specific shape) — used by the
        // traces scatter and the experiment overlays to navigate on click.
        onclick?: (params: { componentType?: string; data?: unknown }) => void;
        id?: string;
    } = $props();

    let el: HTMLDivElement;
    let chart: ReturnType<typeof echarts.init> | null = null;
    let theme = chartTheme();

    function expose() {
        if (!id || !chart) return;
        const w = window as Window & { __tumultCharts?: Record<string, unknown> };
        w.__tumultCharts ??= {};
        w.__tumultCharts[id] = chart.getOption();
    }

    function init() {
        chart?.dispose();
        chart = echarts.init(el, theme);
        chart.setOption(option);
        if (onclick) chart.on('click', onclick);
        expose();
    }

    onMount(() => {
        init();
        const ro = new ResizeObserver(() => chart?.resize());
        ro.observe(el);
        const retheme = () => {
            const next = chartTheme();
            if (next === theme) return;
            theme = next;
            init();
        };
        const mo = new MutationObserver(retheme);
        mo.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });
        const mq = window.matchMedia?.('(prefers-color-scheme: light)');
        mq?.addEventListener?.('change', retheme);
        return () => {
            ro.disconnect();
            mo.disconnect();
            mq?.removeEventListener?.('change', retheme);
            chart?.dispose();
            chart = null;
        };
    });

    $effect(() => {
        if (chart && option) {
            chart.setOption(option, { notMerge: true });
            expose();
        }
    });
</script>

<div bind:this={el} data-chart={id} style="height: {height}px; width: 100%;"></div>
