<script lang="ts">
    // Generic ECharts host: init once with the theme in force, re-set
    // options on change, re-init when the theme flips, resize with the
    // container, dispose on unmount. Charts render to canvas, so a test
    // asserts on the serialised option instead of pixels: when the page
    // was opened with a `window.__tumultCharts` registry already in place
    // (a Playwright init script), a chart with an `id` mirrors its option
    // there. Nothing is mirrored otherwise. A tooltip is HTML next to the
    // canvas and is asserted as such: with a `window.__tumultChartTips`
    // registry in place, a chart with an `id` puts a function there that
    // shows the tooltip of one of its items, as a pointer over it would.
    //
    // `merge` applies option changes on top of the option in force instead
    // of replacing it, so what the user did to the chart (zoom, pan) stays.
    // `onroam` reports the zoom and centre of a graph series after a zoom
    // or pan, at most once a frame; both are also kept here and re-applied
    // when the theme flips, because that re-creates the chart. A change of
    // `viewKey` says the chart now shows something else: the kept view is
    // dropped and the option replaces the one in force.
    import echarts, { chartTheme } from '#lib/echarts.js';
    import type { EChartsCoreOption } from '#lib/echarts.js';
    import { onMount, untrack } from 'svelte';

    let {
        option,
        height = 260,
        onclick,
        ondblclick,
        onroam,
        merge = false,
        viewKey = '',
        id,
        label,
    }: {
        option: EChartsCoreOption;
        height?: number;
        // ECharts 'click' event params (seriesType-specific shape) — used by the
        // traces scatter and the experiment overlays to navigate on click.
        onclick?: (params: { componentType?: string; data?: unknown }) => void;
        ondblclick?: (params: { componentType?: string; data?: unknown }) => void;
        onroam?: (view: RoamView) => void;
        merge?: boolean;
        viewKey?: string;
        id?: string;
        /** Accessible name of the chart. Without one it is not announced as an image. */
        label?: string;
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

    // A wheel or a drag fires `graphroam` many times a frame, and reading
    // the option back is not free: the last event of a frame is handled.
    let roamedSeries: string | null = null;
    let frame = 0;

    function roamed(params: unknown) {
        const moved = (params as { seriesId?: string }).seriesId;
        if (!moved) return;
        roamedSeries = moved;
        if (frame !== 0) return;
        frame = requestAnimationFrame(settleRoam);
    }

    function settleRoam() {
        frame = 0;
        const all = (chart?.getOption().series ?? []) as RoamedSeries[];
        const series = all.find((s) => s.id === roamedSeries);
        if (!series?.id) return;
        view = { seriesId: series.id, zoom: series.zoom ?? 1, center: series.center ?? null };
        expose();
        onroam?.({ zoom: view.zoom, center: view.center });
    }

    function cancelRoam() {
        if (frame === 0) return;
        cancelAnimationFrame(frame);
        frame = 0;
    }

    function restoreView() {
        if (!chart || !view) return;
        chart.setOption({ series: [{ id: view.seriesId, zoom: view.zoom, center: view.center }] });
    }

    function expose() {
        if (!id || !chart) return;
        const registry = (window as Window & { __tumultCharts?: Record<string, unknown> })
            .__tumultCharts;
        if (!registry) return;
        registry[id] = chart.getOption();
    }

    /**
     * An item of the chart: what `showTip` of ECharts takes to name one,
     * or with `cell` the place of a value on the axes of the series,
     * where a pointer would be.
     */
    interface TipTarget {
        seriesIndex: number;
        dataIndex?: number;
        dataType?: 'node' | 'edge';
        cell?: [number, number];
    }

    function showTip(at: TipTarget) {
        if (!chart) return;
        if (!at.cell) return chart.dispatchAction({ type: 'showTip', ...at });
        const [x, y] = chart.convertToPixel({ seriesIndex: at.seriesIndex }, at.cell);
        chart.dispatchAction({ type: 'showTip', x, y });
    }

    function exposeTips() {
        if (!id) return;
        const registry = (
            window as Window & { __tumultChartTips?: Record<string, (at: TipTarget) => void> }
        ).__tumultChartTips;
        if (!registry) return;
        registry[id] = showTip;
    }

    function init() {
        cancelRoam();
        chart?.dispose();
        chart = echarts.init(el, theme);
        chart.setOption(option);
        restoreView();
        if (onclick) chart.on('click', onclick);
        if (ondblclick) chart.on('dblclick', ondblclick);
        chart.on('graphroam', roamed);
        expose();
        exposeTips();
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
            cancelRoam();
            chart?.dispose();
            chart = null;
        };
    });

    let shownKey = untrack(() => viewKey);

    $effect(() => {
        const key = viewKey;
        if (!chart || !option) return;
        const moved = key !== shownKey;
        shownKey = key;
        if (moved) {
            cancelRoam();
            view = null;
        }
        chart.setOption(option, { notMerge: !merge || moved });
        expose();
        if (moved) onroam?.({ zoom: 1, center: null });
    });
</script>

<div
    bind:this={el}
    data-chart={id}
    role={label ? 'img' : undefined}
    aria-label={label || undefined}
    style="height: {height}px; width: 100%;"
></div>
