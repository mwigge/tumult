<script lang="ts">
    // Forecast fan chart: observed line, forecast median and stacked
    // quantile bands (the transparent-lower-series trick), with optional
    // fault windows (markArea) and objective thresholds (markLine). Bands
    // are drawn widest first so narrower ones sit on top.
    import type { EChartsCoreOption } from '#lib/echarts.js';
    import { chartTokens } from '#lib/echarts.js';
    import EChart from './EChart.svelte';

    export interface FanBand {
        /** Coverage level in (0, 1), e.g. 0.8 for the 10–90 % band. */
        level: number;
        lower: (number | null)[];
        upper: (number | null)[];
    }
    export interface FanSeries {
        /** Epoch ms, shared by every array below. */
        ts: number[];
        observed: (number | null)[];
        median?: (number | null)[];
        bands?: FanBand[];
    }

    let {
        series,
        windows = [],
        thresholds = [],
        unit = '',
        height = 260,
        id,
        label,
    }: {
        series: FanSeries;
        windows?: { from: number; to: number; label?: string; color?: string }[];
        thresholds?: { value: number; label?: string }[];
        unit?: string;
        height?: number;
        id?: string;
        /** Accessible name of the chart. */
        label?: string;
    } = $props();

    const t = chartTokens();

    function pairs(values: (number | null)[]): [number, number | null][] {
        return series.ts.map((x, i) => [x, values[i] ?? null]);
    }

    const option: EChartsCoreOption = $derived.by(() => {
        const bands = [...(series.bands ?? [])].sort((a, b) => b.level - a.level);
        const bandSeries = bands.flatMap((b, i) => {
            const alpha = 0.12 + (0.25 * (i + 1)) / Math.max(bands.length, 1);
            const stack = `band-${b.level}`;
            return [
                {
                    name: `q${b.level}-lower`,
                    type: 'line',
                    data: pairs(b.lower),
                    stack,
                    lineStyle: { opacity: 0 },
                    symbol: 'none',
                    silent: true,
                    tooltip: { show: false },
                },
                {
                    name: `${Math.round(b.level * 100)} % band`,
                    type: 'line',
                    data: series.ts.map((x, j) => {
                        const lo = b.lower[j];
                        const hi = b.upper[j];
                        return [x, lo == null || hi == null ? null : hi - lo];
                    }),
                    stack,
                    lineStyle: { opacity: 0 },
                    symbol: 'none',
                    areaStyle: { color: t.accent, opacity: alpha },
                    tooltip: { show: false },
                },
            ];
        });
        const observed = {
            name: 'observed',
            type: 'line',
            data: pairs(series.observed),
            lineStyle: { color: t.text, width: 1.6 },
            itemStyle: { color: t.text },
            symbol: 'none',
            markArea:
                windows.length > 0
                    ? {
                          silent: true,
                          data: windows.map((w) => [
                              {
                                  name: w.label ?? '',
                                  xAxis: w.from,
                                  itemStyle: { color: w.color ?? t.warn, opacity: 0.12 },
                              },
                              { xAxis: w.to },
                          ]),
                      }
                    : undefined,
            markLine:
                thresholds.length > 0
                    ? {
                          silent: true,
                          symbol: 'none',
                          lineStyle: { color: t.fail, type: 'dashed' },
                          label: { formatter: (p: { name?: string }) => p.name ?? '' },
                          data: thresholds.map((th) => ({ name: th.label ?? '', yAxis: th.value })),
                      }
                    : undefined,
        };
        const median = series.median
            ? [
                  {
                      name: 'forecast',
                      type: 'line',
                      data: pairs(series.median),
                      lineStyle: { color: t.accent, width: 1.6, type: 'dashed' },
                      itemStyle: { color: t.accent },
                      symbol: 'none',
                  },
              ]
            : [];
        return {
            grid: { left: 48, right: 16, top: 24, bottom: 28 },
            tooltip: { trigger: 'axis', ...t.tooltip },
            xAxis: { type: 'time' },
            yAxis: { type: 'value', name: unit, scale: true },
            series: [...bandSeries, observed, ...median],
        };
    });
</script>

<EChart {option} {height} {id} {label} />
