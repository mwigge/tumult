<script lang="ts">
    // Generic ECharts host: init once with the theme in force, re-set
    // options on change, re-init when the theme flips, resize with the
    // container, dispose on unmount. Charts render to canvas, so a test
    // asserts on the serialised option instead of pixels: when the page
    // was opened with a `window.__tumultCharts` registry already in place
    // (a Playwright init script), a chart with an `id` mirrors its option
    // there. Nothing is mirrored otherwise. A tooltip is HTML next to the
    // canvas and is asserted as such: next to its option, under
    // `<id>#tip`, the chart puts a function that shows the tooltip of one
    // of its items, as a pointer over it would. That registry is the one
    // hook a test has.
    //
    // `merge` applies option changes on top of the option in force instead
    // of replacing it, so what the user did to the chart (zoom, pan) stays.
    // `onroam` reports the zoom and centre of a graph series after a zoom
    // or pan, at most once a frame; both are also kept here and re-applied
    // when the theme flips, because that re-creates the chart. A change of
    // `viewKey` says the chart now shows something else: the kept view is
    // dropped and the option replaces the one in force.
    //
    // `edgesOf` is for a graph: given the node under the pointer, it names
    // the edges to light up with it (by index), so that their labels are
    // written while the pointer is there. Under `<id>#lit` a test reads
    // which edges are lit. Without it a hover is left to the chart.
    import echarts, { chartTheme, followTheme } from '#lib/echarts.js';
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
        edgesOf,
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
        edgesOf?: (node: HoverParams) => number[];
    } = $props();

    interface HoverParams {
        seriesIndex?: number;
        dataType?: string;
        data?: unknown;
    }

    // Edges lit with the node under the pointer.
    let lit: { seriesIndex: number; dataIndex: number[] } | null = null;

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
        registry[`${id}#tip`] = showTip;
        registry[`${id}#lit`] = lit?.dataIndex ?? [];
    }

    function unlight() {
        if (!lit) return;
        chart?.dispatchAction({ type: 'downplay', dataType: 'edge', ...lit });
        lit = null;
        expose();
    }

    // The chart already dims what is away from the node: the edges only
    // take on their hover look, and nothing else is blurred.
    function light(params: unknown) {
        const p = params as HoverParams;
        unlight();
        if (!edgesOf || p.dataType !== 'node' || p.seriesIndex === undefined) return;
        const dataIndex = edgesOf(p);
        if (dataIndex.length === 0) return;
        lit = { seriesIndex: p.seriesIndex, dataIndex };
        chart?.dispatchAction({ type: 'highlight', dataType: 'edge', notBlur: true, ...lit });
        expose();
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

    function init() {
        cancelRoam();
        chart?.dispose();
        chart = echarts.init(el, theme);
        chart.setOption(option);
        restoreView();
        if (onclick) chart.on('click', onclick);
        if (ondblclick) chart.on('dblclick', ondblclick);
        chart.on('graphroam', roamed);
        lit = null;
        if (edgesOf) {
            chart.on('mouseover', light);
            chart.on('mouseout', unlight);
        }
        expose();
    }

    onMount(() => {
        init();
        const ro = new ResizeObserver(() => chart?.resize());
        ro.observe(el);
        // The first call is the theme init() drew in: only a change draws again.
        const stop = followTheme((next) => {
            if (next === theme) return;
            theme = next;
            init();
        });
        return () => {
            ro.disconnect();
            stop();
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
        unlight();
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
