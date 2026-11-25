<script lang="ts">
    import { page } from '$app/state';
    import { goto } from '$app/navigation';
    import { onMount } from 'svelte';
    import { api } from '#lib/api.js';
    import type { ExperimentWindow, Overview } from '#lib/types.js';
    import { chartTheme, chartTokens, followTheme } from '#lib/echarts.js';
    import type { LegendItem } from '#lib/legend.js';
    import { experimentOverlay, outcomeLegend, overlayRunId } from '#lib/overlays.js';
    import ChartLegend from '#lib/components/ChartLegend.svelte';
    import KpiCard from '#lib/components/KpiCard.svelte';
    import RangeSwitch from '#lib/components/RangeSwitch.svelte';
    import EChart from '#lib/components/EChart.svelte';
    import PageHeader from '#lib/components/PageHeader.svelte';
    import Panel from '#lib/components/Panel.svelte';
    import State from '#lib/components/State.svelte';

    const range = $derived(page.url.searchParams.get('range') ?? '24h');

    let data: Overview | null = $state(null);
    let error: string | null = $state(null);
    // Experiment runs overlapping the visible window (chart overlay bands).
    let runs: ExperimentWindow[] = $state([]);

    // The charts are drawn in the colours of the theme in force.
    let theme = $state(chartTheme());
    onMount(() => followTheme((next) => (theme = next)));
    const tokens = $derived(chartTokens(theme));

    $effect(() => {
        let cancelled = false;
        data = null;
        error = null;
        api.overview(range)
            .then((d) => {
                if (cancelled) return;
                data = d;
                api.experimentWindows(d.from_ns, d.to_ns)
                    .then((w) => !cancelled && (runs = w.runs))
                    .catch(() => !cancelled && (runs = []));
            })
            .catch((e) => !cancelled && (error = String(e)));
        return () => {
            cancelled = true;
        };
    });

    function setRange(r: string) {
        goto(`?range=${r}`, { replace: true, reset: false });
    }

    const dayStr = (ts: number) => new Date(ts * 1000).toISOString().slice(0, 10);

    /** Calendar hues, from no run to the busiest day of the range. */
    const calendarHues = $derived([tokens.hover, tokens.accentDim, tokens.accent]);
    const busiest = $derived.by(() =>
        Math.max(1, ...(data?.experiments_per_day ?? []).map((p) => p.v)),
    );

    // What the colours of the per-day chart stand for: the outcome of the
    // run a band marks, or the two ends of the calendar scale.
    const perDayLegend: LegendItem[] = $derived.by(() => {
        if (range === '24h') {
            return runs.length === 0
                ? []
                : [{ label: 'each band is one run:' }, ...outcomeLegend(tokens)];
        }
        return [
            { label: 'runs per day:' },
            { label: '0', color: calendarHues[0] },
            { label: String(busiest), color: calendarHues[calendarHues.length - 1] },
        ];
    });

    const perDayOption = $derived.by(() => {
        if (!data) return {};
        const points = data.experiments_per_day;
        if (range === '24h') {
            // Category axis of day buckets: overlay bands map a run onto its day
            // column. (The calendar-heatmap branch below has no markArea support
            // on the calendar coordinate system, so overlays are 24h-only here.)
            const overlay = experimentOverlay(runs, tokens, (ms) => dayStr(ms / 1000));
            return {
                ...tooltip(),
                grid: { left: 40, right: 12, top: 26, bottom: 26 },
                xAxis: axisCat(points.map((p) => dayStr(p.ts))),
                yAxis: axisVal('runs'),
                series: [
                    {
                        type: 'bar',
                        data: points.map((p) => p.v),
                        itemStyle: { color: tokens.accent, borderRadius: [3, 3, 0, 0] },
                        barMaxWidth: 44,
                    },
                    ...(overlay ? [overlay] : []),
                ],
            };
        }
        const from = dayStr(data.from_ns / 1_000_000_000);
        const to = dayStr(data.to_ns / 1_000_000_000);
        return {
            tooltip: { ...tokens.tooltip },
            visualMap: {
                min: 0,
                max: busiest,
                show: false,
                inRange: { color: calendarHues },
            },
            calendar: {
                range: [from, to],
                top: 34,
                left: 44,
                right: 12,
                cellSize: ['auto', 16],
                splitLine: { lineStyle: { color: tokens.axis } },
                itemStyle: { color: 'transparent', borderColor: tokens.axis },
                dayLabel: { color: tokens.text },
                monthLabel: { color: tokens.text },
                yearLabel: { show: false },
            },
            series: [
                {
                    type: 'heatmap',
                    coordinateSystem: 'calendar',
                    data: points.map((p) => [dayStr(p.ts), p.v]),
                },
            ],
        };
    });

    const faultsOption = $derived.by(() => {
        if (!data) return {};
        return {
            tooltip: { ...tokens.tooltip, trigger: 'item' },
            legend: { bottom: 0, textStyle: { color: tokens.text } },
            series: [
                {
                    type: 'pie',
                    radius: ['48%', '72%'],
                    center: ['50%', '44%'],
                    itemStyle: { borderColor: tokens.panel, borderWidth: 2 },
                    label: { show: false },
                    data: data.faults.map((f) => ({
                        name: f.fault_subtype
                            ? `${f.fault_type} / ${f.fault_subtype}`
                            : f.fault_type,
                        value: f.count,
                    })),
                    color: [tokens.warn, tokens.accent, tokens.fail, tokens.ok],
                },
            ],
        };
    });

    function tooltip() {
        return { tooltip: { ...tokens.tooltip } };
    }
    function onOverlayClick(params: { componentType?: string; data?: unknown }) {
        const id = overlayRunId(params);
        if (id) goto(`/experiments/${encodeURIComponent(id)}`);
    }
    function axisCat(data: string[]) {
        return {
            type: 'category',
            data,
            axisLine: { lineStyle: { color: tokens.axis } },
            axisLabel: { color: tokens.text },
            axisTick: { show: false },
        };
    }
    function axisVal(name: string) {
        return {
            type: 'value',
            name,
            nameTextStyle: { color: tokens.text },
            minInterval: 1,
            splitLine: { lineStyle: { color: tokens.split } },
            axisLabel: { color: tokens.text },
        };
    }
</script>

<PageHeader title="Overview" sub="resilience for the selected window">
    <div style="margin-left: auto">
        <RangeSwitch value={range} onchange={setRange} />
    </div>
</PageHeader>

{#if error}
    <State kind="error" panel>Failed to load overview: {error}</State>
{:else if !data}
    <div
        class="grid"
        style="grid-template-columns: repeat(5, 1fr); margin-bottom: var(--space-3-5)"
    >
        {#each [0, 1, 2, 3, 4] as i (i)}
            <State kind="loading" height={92} />
        {/each}
    </div>
    <State kind="loading" height={260} />
{:else}
    <div class="grid kpis">
        {#each data.kpis as kpi (kpi.name)}
            <KpiCard {kpi} />
        {/each}
    </div>

    <div class="grid cols-2" style="margin-top: var(--space-3-5)">
        <Panel title="Runs per day">
            {#if data.experiments_per_day.length === 0}
                <State kind="empty">No runs in this window.</State>
            {:else}
                <EChart
                    id="per-day"
                    option={perDayOption}
                    label={range === '24h'
                        ? 'Count of runs per day. The bands and lines mark single runs in the colour of their outcome.'
                        : 'Count of runs per day as a calendar.'}
                    height={range === '24h' ? 220 : 200}
                    onclick={onOverlayClick}
                />
                {#if perDayLegend.length > 0}
                    <ChartLegend id="per-day" items={perDayLegend} />
                {/if}
            {/if}
        </Panel>
        <Panel title="Fault injections">
            {#if data.faults.length === 0}
                <State kind="empty">No fault injection step was recorded in this window.</State>
            {:else}
                <EChart
                    id="faults"
                    option={faultsOption}
                    height={220}
                    label="Fault injection steps per kind and plugin"
                />
            {/if}
        </Panel>
    </div>

    <Panel title="Target systems" style="margin-top: var(--space-3-5)">
        {#if data.targets.length === 0}
            <State kind="empty">
                No target-system metadata in this window — tumult runs don't tag target systems yet,
                so this fills in once targets are annotated.
            </State>
        {:else}
            {@const max = Math.max(...data.targets.map((t) => t.experiments))}
            <table class="data">
                <thead>
                    <tr
                        ><th>Target</th><th style="width: 40%">Experiments</th><th>n</th><th
                            >Pass rate</th
                        ></tr
                    >
                </thead>
                <tbody>
                    {#each data.targets as t (t.target)}
                        <tr>
                            <td class="mono">{t.target}</td>
                            <td>
                                <div
                                    class="tbar"
                                    style="width: {(t.experiments / max) * 100}%"
                                ></div>
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
    </Panel>
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
        border-radius: var(--radius-xs);
        background: var(--accent-dim);
        min-width: 2px;
    }
</style>
