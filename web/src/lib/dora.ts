// Chart options and formatters for the DORA score view. Pure functions of
// the API documents and the theme, so the page stays markup and the
// options can be asserted through `window.__tumultCharts`.

import type { ChartTheme, EChartsCoreOption } from './echarts';
import { chartTokens } from './echarts';
import type { LegendItem } from './legend';
import { strong, tip, tipFormatter } from './tooltip';
import type { Tip } from './tooltip';
import type {
    DoraCell,
    DoraReport,
    MaturityHistoryPoint,
    MaturityComponent,
    MaturityNode,
    MaturityReport,
    ProofDays,
} from './types';

export type HeatmapMode = 'coverage' | 'status';

/** Coverage hues (Okabe-Ito, colour-blind safe), low to high. */
const COVERAGE_HUES = ['#D55E00', '#E69F00', '#009E73'] as const;
/** Evidence status hues by rank: none, designed, existing, operating. */
const STATUS_HUES = ['#D55E00', '#E69F00', '#56B4E9', '#009E73'] as const;
export const STATUS_NAMES = ['none', 'designed', 'existing', 'operating'] as const;
/** Coverage at or above which a cell takes the high hue, and the middle one. */
const COVERAGE_STOPS = [0.5, 0.8] as const;

export const COMPONENTS: { key: MaturityComponent; letter: string; label: string }[] = [
    { key: 'conformance', letter: 'C', label: 'Conformance' },
    { key: 'coverage', letter: 'T', label: 'Test coverage' },
    { key: 'slo', letter: 'S', label: 'SLO attainment' },
    { key: 'recovery', letter: 'R', label: 'Recovery' },
    { key: 'loop', letter: 'L', label: 'Learning loop' },
];

/** `0.071` → `7 %`. */
export const pct = (share: number) => `${Math.round(share * 100)} %`;

/** The label every number carries: how much of it rests on nothing. */
export const unknownLabel = (share: number) => `unknown ${pct(share)}`;

/** A coverage value for a tile; `null` is unknown, never zero. */
export const coverageText = (coverage: number | null) =>
    coverage === null ? 'unknown' : pct(coverage);

export function coverageHue(coverage: number): string {
    if (coverage >= COVERAGE_STOPS[1]) return COVERAGE_HUES[2];
    if (coverage >= COVERAGE_STOPS[0]) return COVERAGE_HUES[1];
    return COVERAGE_HUES[0];
}

export const statusHue = (rank: number) =>
    STATUS_HUES[Math.max(0, Math.min(rank, STATUS_HUES.length - 1))];

/** The signs every heatmap cell can carry, in either view. */
const CELL_SIGNS: LegendItem[] = [
    { label: 'hatched = unknown share', mark: 'hatch' },
    { label: 'no source connected', glyph: '?' },
    { label: 'no events in window', glyph: '–' },
];

/**
 * The legend of the heatmap: what a cell holds, each hue with the
 * coverage from which it is drawn (or the evidence status it stands for),
 * and the signs of an unknown share and of a cell without events.
 */
export function heatmapLegend(mode: HeatmapMode): LegendItem[] {
    if (mode === 'status') {
        return [
            { label: 'cell = evidence status' },
            ...STATUS_NAMES.map((name, rank) => ({
                label: `${rank} ${name}`,
                color: statusHue(rank),
            })),
            ...CELL_SIGNS,
        ];
    }
    const [middle, high] = COVERAGE_STOPS;
    return [
        { label: 'cell = coverage in %' },
        { label: `below ${pct(middle)}`, color: COVERAGE_HUES[0] },
        { label: `from ${pct(middle)}`, color: COVERAGE_HUES[1] },
        { label: `from ${pct(high)}`, color: COVERAGE_HUES[2] },
        ...CELL_SIGNS,
    ];
}

/** What a heatmap cell shows: its fill, its text and its hatched share. */
export interface CellMark {
    fill: string | null;
    text: string;
    /** Share of the cell width drawn hatched (0–1). */
    hatch: number;
}

export function cellMark(cell: DoraCell, mode: HeatmapMode): CellMark {
    if (cell.state === 'unknown') return { fill: null, text: '?', hatch: 1 };
    if (cell.state === 'no_events') return { fill: null, text: '–', hatch: 0 };
    if (mode === 'status') {
        return {
            fill: statusHue(cell.status_rank),
            text: String(cell.status_rank),
            hatch: cell.unknown,
        };
    }
    const coverage = cell.coverage ?? 0;
    return {
        fill: coverageHue(coverage),
        text: String(Math.round(coverage * 100)),
        hatch: cell.unknown,
    };
}

/** A diagonal-line pattern for the hatched share of a cell. */
function hatchPattern(color: string): unknown {
    if (typeof document === 'undefined') return color;
    const size = 6;
    const canvas = document.createElement('canvas');
    canvas.width = size;
    canvas.height = size;
    const ctx = canvas.getContext('2d');
    if (!ctx) return color;
    ctx.strokeStyle = color;
    ctx.lineWidth = 1.2;
    ctx.beginPath();
    ctx.moveTo(0, size);
    ctx.lineTo(size, 0);
    ctx.stroke();
    return { image: canvas, repeat: 'repeat' };
}

const STATE_TEXT: Record<string, string> = {
    known: '',
    unknown: 'unknown: no source connected for this evidence',
    no_events: 'no events in window',
};

/** What a cell says in words: the tooltip and the table read the same. */
export function cellLines(cell: DoraCell): string[] {
    const lines: string[] = [];
    if (cell.state === 'known') {
        lines.push(`coverage ${coverageText(cell.coverage)} (${unknownLabel(cell.unknown)})`);
        lines.push(`evidence status ${cell.status} (${cell.status_rank} of 3)`);
    } else {
        lines.push(STATE_TEXT[cell.state] ?? cell.state);
    }
    if (cell.gaps > 0) lines.push(`${cell.gaps} requirement${cell.gaps === 1 ? '' : 's'} short`);
    return lines;
}

/** The acts the API names by a key that is not an article number. */
const ACTS: ReadonlyMap<string, string> = new Map([['rts1774', 'RTS 2024/1774']]);

/** How an article of the API reads: `Art. 11`, or the act of the RTS rows, `RTS 2024/1774`. */
const articleLabel = (article: string) => ACTS.get(article) ?? `Art. ${article}`;

/** The heatmap tooltip of a cell. */
function cellTooltip(cell: DoraCell & { row: string }): Tip {
    return tip([strong(articleLabel(cell.article)), ` · ${cell.row}`], ...cellLines(cell));
}

/** The cell of an article and a row, when that article applies to it. */
export const cellOf = (report: DoraReport, article: string, entityId: string) =>
    report.cells.find((c) => c.article === article && c.entity_id === entityId) ?? null;

interface RenderApi {
    value: (dim: number) => number;
    coord: (point: number[]) => number[];
    size: (span: number[]) => number[];
}

/**
 * Articles × functions. Hue is coverage (or the evidence status rank),
 * the hatched part of a cell is its unknown share, a dash is "no events in
 * the window". Rows that do not apply to the profile are not drawn.
 */
export function heatmapOption(
    report: DoraReport,
    mode: HeatmapMode,
    theme: ChartTheme,
): EChartsCoreOption {
    const t = chartTokens(theme);
    const articles = report.articles.map((a) => a.article);
    const rows = report.rows;
    const hatch = hatchPattern(t.text);
    const data = report.cells
        .map((cell) => {
            const x = articles.indexOf(cell.article);
            const y = rows.findIndex((r) => r.entity_id === cell.entity_id);
            const mark = cellMark(cell, mode);
            return { ...cell, ...mark, row: rows[y]?.label ?? cell.entity_id, value: [x, y] };
        })
        .filter((d) => d.value[0] >= 0 && d.value[1] >= 0);
    return {
        tooltip: {
            ...t.tooltip,
            className: 'dora-tooltip',
            formatter: tipFormatter((p: { data?: DoraCell & { row: string } }) =>
                p.data ? cellTooltip(p.data) : null,
            ),
        },
        grid: { left: 8, right: 12, top: 34, bottom: 8, containLabel: true },
        xAxis: {
            type: 'category',
            position: 'top',
            data: articles.map(articleLabel),
            splitArea: { show: false },
            axisTick: { show: false },
        },
        yAxis: {
            type: 'category',
            inverse: true,
            data: rows.map((r) => r.label),
            axisTick: { show: false },
        },
        series: [
            {
                type: 'custom',
                name: mode,
                data,
                renderItem: (params: { dataIndex: number }, api: RenderApi) => {
                    const d = data[params.dataIndex];
                    const [cx, cy] = api.coord([api.value(0), api.value(1)]);
                    const [bw, bh] = api.size([1, 1]);
                    const [w, h] = [bw - 3, bh - 3];
                    const [x, y] = [cx - w / 2, cy - h / 2];
                    const hatched = w * d.hatch;
                    return {
                        type: 'group',
                        children: [
                            {
                                type: 'rect',
                                shape: { x, y, width: w, height: h, r: 2 },
                                style: {
                                    fill: d.fill ?? 'transparent',
                                    stroke: t.axis,
                                    lineWidth: d.fill ? 0 : 1,
                                },
                            },
                            {
                                type: 'rect',
                                shape: { x: x + w - hatched, y, width: hatched, height: h },
                                style: { fill: hatch, opacity: d.fill ? 0.75 : 0.5 },
                                invisible: hatched <= 0,
                            },
                            {
                                type: 'text',
                                style: {
                                    text: d.text,
                                    x: cx,
                                    y: cy,
                                    align: 'center',
                                    verticalAlign: 'middle',
                                    fill: d.fill ? '#0b0e11' : t.text,
                                    fontSize: 11,
                                    fontWeight: 600,
                                },
                            },
                        ],
                    };
                },
            },
        ],
    };
}

/** Height that keeps heatmap rows readable. */
export const heatmapHeight = (rows: number) => 70 + Math.max(rows, 1) * 34;

/**
 * The five maturity components with their profile weights. A component
 * without a value has no bar: its label says why.
 */
export function componentsOption(m: MaturityReport, theme: ChartTheme): EChartsCoreOption {
    const t = chartTokens(theme);
    const names = COMPONENTS.map(
        (c) => `${c.label} (${c.letter}) · weight ${m.weights[c.key].toFixed(2)}`,
    );
    const capped = m.cap.component;
    const data = COMPONENTS.map((c) => {
        const value = m.components[c.key];
        const state = m.component_states[c.key]?.state ?? 'unknown';
        return {
            value,
            component: c.key,
            state,
            itemStyle: { color: capped === c.key ? t.warn : t.accent },
            label: {
                show: true,
                position: value === null ? 'insideLeft' : 'right',
                color: t.text,
                formatter: value === null ? state.replaceAll('_', ' ') : value.toFixed(1),
            },
        };
    });
    return {
        tooltip: { ...t.tooltip, trigger: 'item' },
        // The grid holds the labels, not the axis name: room for it below.
        grid: { left: 8, right: 48, top: 8, bottom: 32, containLabel: true },
        xAxis: {
            type: 'value',
            min: 0,
            max: 100,
            name: 'score (0 to 100)',
            nameLocation: 'middle',
            nameGap: 24,
            nameTextStyle: { color: t.text },
        },
        yAxis: { type: 'category', inverse: true, data: names, axisTick: { show: false } },
        series: [{ type: 'bar', name: 'components', barMaxWidth: 16, data }],
    };
}

/** The legend of the bars: the colour of a score, and of the component that caps the level. */
export function componentsLegend(theme: ChartTheme): LegendItem[] {
    const t = chartTokens(theme);
    return [
        { label: 'component score', color: t.accent },
        { label: 'the component that caps the level', color: t.warn },
    ];
}

/**
 * The level gauge: the pointer is the index on the 1–5 house scale. When
 * a gate caps the level the index is greyed, because the level no longer
 * follows it.
 */
export function gaugeOption(m: MaturityReport, theme: ChartTheme): EChartsCoreOption {
    const t = chartTokens(theme);
    const color = m.capped ? t.axis : t.accent;
    const reached = (m.level - 1) / 4;
    const target = (m.target - 1) / 4;
    return {
        series: [
            {
                type: 'gauge',
                name: 'maturity',
                min: 1,
                max: 5,
                splitNumber: 4,
                startAngle: 200,
                endAngle: -20,
                radius: '95%',
                center: ['50%', '62%'],
                axisLine: {
                    lineStyle: {
                        width: 12,
                        color: [
                            [Math.min(reached, target), t.ok],
                            [Math.max(reached, target), reached < target ? t.warn : t.ok],
                            [1, t.split],
                        ],
                    },
                },
                pointer: { length: '58%', width: 4, itemStyle: { color } },
                anchor: { show: true, size: 8, itemStyle: { color } },
                axisTick: { show: false },
                splitLine: { length: 12, lineStyle: { color: t.axis } },
                axisLabel: { color: t.text, distance: 18, fontSize: 11 },
                title: { show: false },
                detail: {
                    valueAnimation: false,
                    offsetCenter: [0, '38%'],
                    fontSize: 15,
                    color: m.capped ? t.text : t.accent,
                    formatter: (v: number) => `index ${v.toFixed(2)}`,
                },
                data: [{ value: m.index, level: m.level, target: m.target, capped: m.capped }],
            },
        ],
    };
}

/** The names of the two lines of the history, in the chart and in its legend. */
const HISTORY_LINES = { level: 'Level', index: 'Index' } as const;

/** Level and index per quarter. */
export function historyOption(m: MaturityReport, theme: ChartTheme): EChartsCoreOption {
    const t = chartTokens(theme);
    return {
        tooltip: { ...t.tooltip, trigger: 'axis' },
        // The grid holds the labels, not the axis name: room for it above.
        grid: { left: 8, right: 12, top: 28, bottom: 8, containLabel: true },
        xAxis: { type: 'category', data: m.history.map((h) => h.quarter) },
        yAxis: {
            type: 'value',
            min: 1,
            max: 5,
            interval: 1,
            name: 'level (1 to 5)',
            nameTextStyle: { color: t.text, align: 'left' },
        },
        series: [
            {
                type: 'line',
                name: HISTORY_LINES.level,
                step: 'end',
                symbol: 'circle',
                data: m.history.map((h) => h.level),
                lineStyle: { color: t.accent },
                itemStyle: { color: t.accent },
            },
            {
                type: 'line',
                name: HISTORY_LINES.index,
                symbol: 'circle',
                data: m.history.map((h) => h.index),
                lineStyle: { color: t.text, type: 'dashed' },
                itemStyle: { color: t.text },
            },
        ],
    };
}

/** The legend of the history: each line in the colour and the stroke it is drawn in. */
export function historyLegend(theme: ChartTheme): LegendItem[] {
    const t = chartTokens(theme);
    return [
        { label: HISTORY_LINES.level, color: t.accent, mark: 'line' },
        { label: HISTORY_LINES.index, color: t.text, mark: 'dashed' },
    ];
}

/** How a quarter of the history reads in the table next to the chart. */
export function historyStateText(point: MaturityHistoryPoint): string {
    if (point.state === 'today') return 'today';
    if (point.state !== 'stored') return 'no data';
    const stored = `stored ${point.as_of ?? ''}`.trim();
    return point.source === 'proof' ? `${stored} by the proof tool` : stored;
}

/** How many days the daily job left out, in words; nothing before the first. */
export function snapshotSkippedText(skipped: number): string {
    if (skipped <= 0) return '';
    return `${skipped} day${skipped === 1 ? '' : 's'} skipped since the daemon started`;
}

/** What the proof days in the lake amount to, in words. */
export function proofDaysText(proof: ProofDays): string {
    if (proof.enabled) return `${proof.counted} proof day${proof.counted === 1 ? '' : 's'} counted`;
    if (proof.ignored <= 0) return '';
    return proof.ignored === 1
        ? '1 day stored by the proof tool is not counted'
        : `${proof.ignored} days stored by the proof tool are not counted`;
}

/** How a node's level reads: with the nodes that cap it, when one does. */
export function capText(node: MaturityNode): string {
    if (node.cap.origin.length > 0) {
        return `level ${node.level}, capped by ${node.cap.origin.join(', ')}`;
    }
    if (node.cap.reason) return `level ${node.level}, held by: ${node.cap.reason}`;
    return `level ${node.level}`;
}

/** Nodes in tree order with their depth, for an indented table. */
export function nodeTree(nodes: MaturityNode[]): { node: MaturityNode; depth: number }[] {
    const ids = new Set(nodes.map((n) => n.id));
    const children = new Map<string, MaturityNode[]>();
    const roots: MaturityNode[] = [];
    for (const node of nodes) {
        if (node.parent === null || !ids.has(node.parent)) {
            roots.push(node);
            continue;
        }
        const siblings = children.get(node.parent) ?? [];
        siblings.push(node);
        children.set(node.parent, siblings);
    }
    const out: { node: MaturityNode; depth: number }[] = [];
    const walk = (node: MaturityNode, depth: number) => {
        out.push({ node, depth });
        for (const child of children.get(node.id) ?? []) walk(child, depth + 1);
    };
    for (const root of roots) walk(root, 0);
    return out;
}

/** `3600` → `1 h`; `300` → `5 min`. */
export function durationText(seconds: number | null): string {
    if (seconds === null) return '–';
    if (seconds >= 3600 && seconds % 3600 === 0) return `${seconds / 3600} h`;
    if (seconds >= 60) return `${Math.round(seconds / 60)} min`;
    return `${seconds} s`;
}
