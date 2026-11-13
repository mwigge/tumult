<script lang="ts">
    // Generic ECharts host: init once with the theme in force, re-set
    // options on change, re-init when the theme flips, resize with the
    // container, dispose on unmount. With an `id`, the option in force is
    // mirrored to `window.__tumultCharts[id]` so Playwright can assert on
    // the serialised option instead of pixels (charts render to canvas).
    //
    // `merge` applies option changes on top of the option in force instead
    // of replacing it, so what the user did to the chart (zoom, pan) stays.
    // `onroam` reports the zoom and centre of a graph series after each
    // zoom or pan; both are also kept here and re-applied when the theme
    // flips, because that re-creates the chart.
    import echarts, { chartTheme } from '#lib/echarts.js';
    import type { EChartsCoreOption } from '#lib/echarts.js';
    import { onMount } from 'svelte';

    let {
        option,
        height = 260,
        onclick,
        ondblclick,
        onroam,
        merge = false,
        id,
    }: {
        option: EChartsCoreOption;
        height?: number;
        // ECharts 'click' event params (seriesType-specific shape) — used by the
        // traces scatter and the experiment overlays to navigate on click.
        onclick?: (params: { componentType?: string; data?: unknown }) => void;
        ondblclick?: (params: { componentType?: string; data?: unknown }) => void;
        onroam?: (view: RoamView) => void;
        merge?: boolean;
        id?: string;
    } = $props();

    let el: HTMLDivElement;
    let chart: ReturnType<typeof echarts.init> | null = null;
    let theme = chartTheme();
    // Zoom and centre of the graph series the user last moved.
    let view: (RoamView & { seriesId: string }) | null = null;

    interface RoamView {
        zoom: number;
        center: number[] | null;
    }

    interface RoamedSeries {
        id?: string;
        zoom?: number;
        center?: number[] | null;
    }

    function roamed(params: unknown) {
        const moved = (params as { seriesId?: string }).seriesId;
        const all = (chart?.getOption().series ?? []) as RoamedSeries[];
        const series = all.find((s) => s.id === moved);
        if (!series?.id) return;
        view = { seriesId: series.id, zoom: series.zoom ?? 1, center: series.center ?? null };
        expose();
        onroam?.({ zoom: view.zoom, center: view.center });
    }

    function restoreView() {
        if (!chart || !view) return;
        chart.setOption({ series: [{ id: view.seriesId, zoom: view.zoom, center: view.center }] });
    }

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
        restoreView();
        if (onclick) chart.on('click', onclick);
        if (ondblclick) chart.on('dblclick', ondblclick);
        chart.on('graphroam', roamed);
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
            chart.setOption(option, { notMerge: !merge });
            expose();
        }
    });
</script>

<div bind:this={el} data-chart={id} style="height: {height}px; width: 100%;"></div>
