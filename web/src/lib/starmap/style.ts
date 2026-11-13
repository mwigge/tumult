// Star Map styling: pure functions from an API node or edge to the ECharts
// item that draws it. A lens changes fill and tooltip, never the node set,
// the shape or the position. Every colour comes from the chart tokens of
// the theme in force or from the categorical palette; status is always
// double-encoded (fill plus border style) so it survives greyscale.

import { PALETTE } from '../echarts.js';
import type { chartTokens } from '../echarts.js';
import type { StarEdge, StarKind, StarNode, StarRel } from '../types.js';
import type { Point } from './layout.js';

export type Lens = 'evidence' | 'risk' | 'conformance';
export const LENSES: Lens[] = ['evidence', 'risk', 'conformance'];

export type Tokens = ReturnType<typeof chartTokens>;

export interface StyleContext {
    lens: Lens;
    tokens: Tokens;
    /** Id of the selected node, if any. */
    selected: string | null;
    /** Current zoom factor of the graph, for level-of-detail labels. */
    zoom: number;
    nowNs: number;
}

/** Categories of the graph series, in ring order. */
export const KINDS: StarKind[] = [
    'org',
    'entity',
    'system',
    'component',
    'service',
    'workload',
    'ext',
    'def',
    'run',
    'slo',
    'article',
    'gap',
    'cluster',
];

const SHAPE: Record<StarKind, string> = {
    org: 'roundRect',
    entity: 'diamond',
    system: 'rect',
    component: 'rect',
    service: 'circle',
    workload: 'triangle',
    ext: 'pin',
    def: 'circle',
    run: 'emptyCircle',
    slo: 'arrow',
    article: 'rect',
    gap: 'circle',
    cluster: 'roundRect',
};

const SIZE: Record<StarKind, number> = {
    org: 26,
    entity: 20,
    system: 18,
    component: 13,
    service: 14,
    workload: 12,
    ext: 18,
    def: 10,
    run: 8,
    slo: 10,
    article: 8,
    gap: 10,
    cluster: 24,
};

// Palette slot per kind. Slot 6 (yellow) is never used: it vanishes on white.
const HUE: Record<StarKind, number> = {
    org: 0,
    entity: 3,
    system: 4,
    component: 4,
    service: 0,
    workload: 3,
    ext: 1,
    def: 2,
    run: 2,
    slo: 5,
    article: 1,
    gap: 5,
    cluster: 0,
};

/** Smallest zoom at which a kind is labelled; absent kinds always are. */
const LABEL_FROM: Partial<Record<StarKind, number>> = {
    system: 1,
    component: 2,
    service: 2,
    workload: 2,
    ext: 2,
    def: 2,
    run: 2,
    slo: 2,
    article: 2,
    gap: 2,
};

const CONTAINMENT = new Set<StarRel>(['owns', 'implements', 'runs_on']);
const ON_SELECTION = new Set<StarRel>(['tested_by', 'covers', 'measures']);
const SOON_NS = 14 * 86_400_000_000_000;
const DIMMED = 0.35;

const HTML: Record<string, string> = {
    '&': '&amp;',
    '<': '&lt;',
    '>': '&gt;',
    '"': '&quot;',
    "'": '&#39;',
};

/** Escape text for an ECharts tooltip, which renders formatter output as HTML. */
export function esc(text: unknown): string {
    return String(text ?? '').replace(/[&<>"']/g, (c) => HTML[c]);
}

export interface ItemStyle {
    color: string;
    borderColor?: string;
    borderWidth?: number;
    borderType?: string | number[];
    opacity?: number;
}

export interface StarItem {
    id: string;
    name: string;
    x: number;
    y: number;
    category: StarKind;
    symbol: string;
    symbolSize: number;
    itemStyle: ItemStyle;
    label: { show: boolean };
    raw: StarNode;
}

export interface StarLink {
    id: string;
    source: string;
    target: string;
    symbol: [string, string];
    lineStyle: { color: string; width: number; type: string; opacity: number };
    raw: StarEdge;
}

/**
 * The label drawn next to a node. ECharts reads `{name|text}` in a label
 * as rich text; a zero-width space after every opening brace keeps a
 * name with braces in it literal without changing what is seen.
 */
export function labelText(n: StarNode): string {
    return n.label.replace(/\{/g, '{\u200b');
}

export function labelShown(kind: StarKind, zoom: number): boolean {
    return zoom >= (LABEL_FROM[kind] ?? 0);
}

const severe = (worst: string | null) => worst === 'critical' || worst === 'high';

function riskColor(n: StarNode, t: Tokens): string | null {
    if (n.risks.open <= 0) return null;
    return severe(n.risks.worst) ? t.fail : t.warn;
}

function conformanceColor(n: StarNode, t: Tokens): string | null {
    if (n.conformance === 'spine') return t.ok;
    if (n.conformance === 'target') return t.warn;
    if (n.conformance === 'none') return t.fail;
    return null;
}

function evidenceColor(n: StarNode, t: Tokens): string | null {
    if (n.evidence.failed > 0) return t.fail;
    if (n.evidence.runs + n.evidence.manual > 0) return t.ok;
    return null;
}

function fill(n: StarNode, c: StyleContext): string {
    const byLens =
        c.lens === 'risk'
            ? riskColor(n, c.tokens)
            : c.lens === 'conformance'
              ? conformanceColor(n, c.tokens)
              : evidenceColor(n, c.tokens);
    return byLens ?? PALETTE[HUE[n.kind]];
}

function riskBorder(n: StarNode, t: Tokens): Partial<ItemStyle> {
    const color = riskColor(n, t);
    return color ? { borderColor: color, borderWidth: 1.5 } : {};
}

function gapStyle(n: StarNode, t: Tokens): Partial<ItemStyle> {
    if (n.gap === 'missing' || n.kind === 'gap') {
        return {
            color: 'transparent',
            borderColor: t.text,
            borderWidth: 1.5,
            borderType: 'dashed',
        };
    }
    if (n.gap === 'stale') return { borderColor: t.warn, borderWidth: 1.5, borderType: [2, 3] };
    return {};
}

/** Every risk on the node is suppressed: dimmed, never absent. */
function suppressedStyle(n: StarNode, c: StyleContext): Partial<ItemStyle> {
    if (n.risks.open > 0 || n.risks.suppressed <= 0) return {};
    const expires = n.risks.suppression_expires_ns;
    if (expires !== null && expires - c.nowNs <= SOON_NS) {
        return {
            opacity: DIMMED,
            borderColor: c.tokens.warn,
            borderWidth: 1.5,
            borderType: 'dotted',
        };
    }
    return { opacity: DIMMED, borderColor: c.tokens.axis, borderWidth: 1.5 };
}

function selectedStyle(n: StarNode, c: StyleContext): Partial<ItemStyle> {
    return n.id === c.selected ? { borderColor: c.tokens.accent, borderWidth: 3 } : {};
}

function size(n: StarNode): number {
    const base = SIZE[n.kind];
    if (n.kind === 'cluster') return base + Math.min(16, Math.log2(1 + (n.members ?? 0)) * 3);
    return base + Math.min(6, Math.max(0, n.criticality_weight - 1) * 3);
}

/** The graph data item for one node at its layout position. */
export function toItem(n: StarNode, at: Point, c: StyleContext): StarItem {
    return {
        id: n.id,
        name: n.id,
        x: at[0],
        y: at[1],
        category: n.kind,
        symbol: SHAPE[n.kind],
        symbolSize: size(n),
        itemStyle: {
            color: fill(n, c),
            ...riskBorder(n, c.tokens),
            ...gapStyle(n, c.tokens),
            ...suppressedStyle(n, c),
            ...selectedStyle(n, c),
        },
        label: { show: n.id === c.selected || labelShown(n.kind, c.zoom) },
        raw: n,
    };
}

function lineStyle(e: StarEdge, c: StyleContext): StarLink['lineStyle'] {
    const t = c.tokens;
    if (CONTAINMENT.has(e.rel)) return { color: t.axis, width: 0.6, type: 'solid', opacity: 0.5 };
    if (e.rel === 'depends_on') return { color: t.axis, width: 1, type: 'dotted', opacity: 0.7 };
    if (e.rel === 'broke') return { color: t.fail, width: 1.5, type: 'solid', opacity: 0.8 };
    if (ON_SELECTION.has(e.rel)) {
        const shown = c.selected !== null && (e.from === c.selected || e.to === c.selected);
        return { color: t.accent, width: 1, type: 'dashed', opacity: shown ? 0.8 : 0 };
    }
    const width = 1 + Math.min(3, Math.log10(1 + (e.weight ?? 0)));
    return { color: t.axis, width, type: 'solid', opacity: 0.6 };
}

/** The graph link for one edge; the relation decides the line. */
export function toLink(e: StarEdge, c: StyleContext): StarLink {
    return {
        id: e.id,
        source: e.from,
        target: e.to,
        symbol: CONTAINMENT.has(e.rel) ? ['none', 'none'] : ['none', 'arrow'],
        lineStyle: lineStyle(e, c),
        raw: e,
    };
}

const day = (ns: number) => new Date(ns / 1_000_000).toISOString().slice(0, 10);

function suppressionLine(n: StarNode): string | null {
    if (n.risks.suppressed <= 0) return null;
    const who = esc(n.risks.suppressed_by ?? 'unknown');
    const until =
        n.risks.suppression_expires_ns === null
            ? 'with no expiry'
            : `until ${day(n.risks.suppression_expires_ns)}`;
    return `suppressed by ${who} ${until}: ${esc(n.risks.suppression_reason ?? '')}`;
}

function riskLine(n: StarNode): string | null {
    if (n.risks.open + n.risks.suppressed <= 0) return null;
    const worst = n.risks.worst ? ` (worst ${esc(n.risks.worst)})` : '';
    return `risks: ${n.risks.open} open${worst}, ${n.risks.suppressed} suppressed`;
}

function gapLines(n: StarNode): string[] {
    if (!n.gap) return [];
    const wanted = n.gap === 'missing' ? 'missing' : 'stale';
    const names = Object.entries(n.evidence.by_discipline)
        .filter(([, status]) => status === wanted)
        .map(([discipline]) => esc(discipline));
    const head = n.gap === 'missing' ? 'no evidence' : 'stale evidence';
    const lines = [names.length ? `${head}: ${names.join(', ')}` : head];
    if (n.risks.recommended.length) {
        lines.push(`recommended: ${n.risks.recommended.map(esc).join(', ')}`);
    }
    return lines;
}

function evidenceLine(n: StarNode): string | null {
    const e = n.evidence;
    if (e.runs + e.manual <= 0) return null;
    const last = e.last_run_ns === null ? '' : `, last ${day(e.last_run_ns)}`;
    return `${e.runs} runs (${e.failed} failed), ${e.manual} manual${last}`;
}

function headLine(n: StarNode): string {
    const kind = n.kind === 'org' && n.org_kind ? n.org_kind : n.kind;
    const env = n.env ? ` · ${esc(n.env)}` : '';
    return `<b>${esc(n.label)}</b> (${esc(kind)}${env})`;
}

const FLAG_TEXT: Record<string, string> = {
    malformed_id: 'the entity id does not follow the agreed form',
    multiple_owners: 'telemetry reports more than one owner',
    owner_conflict: 'telemetry and org.yaml name different owners',
};

/** What a flag on a node means, in words; an unknown flag is shown as is. */
export function flagText(flag: string): string {
    return FLAG_TEXT[flag] ?? flag;
}

const count = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/**
 * What hangs below an entity or an org unit. An entity is a gap only when
 * every system below it is, so the first line says how many it hides.
 */
export function rollupLines(n: StarNode): string[] {
    const r = n.rollup;
    if (!r) return [];
    const stale = r.stale_systems > 0 ? `, ${r.stale_systems} stale` : '';
    return [
        `${r.untested_systems} of ${count(r.systems, 'system')} untested${stale}`,
        `below: ${count(r.gaps, 'gap')}, ${count(r.failed, 'failed run')}, ` +
            `${count(r.risks_open, 'open risk')}, ${r.risks_suppressed} suppressed`,
    ];
}

function detailLines(n: StarNode): string[] {
    const lines: string[] = [];
    if (n.tier) lines.push(`tier: ${esc(n.tier)}`);
    if (n.posture) lines.push(`posture: ${n.posture.score.toFixed(1)} (${esc(n.posture.band)})`);
    if (n.conflict) lines.push('reports more than one entity id');
    for (const flag of n.flags) lines.push(esc(flagText(flag)));
    lines.push(...rollupLines(n));
    if (n.declared) lines.push('declared only, not seen in telemetry');
    if (n.members !== null) lines.push(`${n.members} nodes collapsed; click to expand`);
    return lines;
}

/** Tooltip body of the active lens. Every name is escaped. */
export function tooltipHtml(n: StarNode, c: StyleContext): string {
    const lines = [headLine(n), ...detailLines(n)];
    if (c.lens === 'conformance' && n.kind === 'service') {
        lines.push(`conformance: ${esc(n.conformance ?? 'unknown')}`);
    }
    lines.push(evidenceLine(n) ?? '', ...gapLines(n), riskLine(n) ?? '', suppressionLine(n) ?? '');
    return lines.filter((line) => line !== '').join('<br/>');
}

const REL_TEXT: Record<StarRel, string> = {
    owns: 'owns',
    implements: 'implements',
    calls: 'calls',
    runs_on: 'runs on',
    depends_on: 'depends on',
    tested_by: 'tested by',
    broke: 'broke',
    covers: 'covers',
    measures: 'measures',
};

/** Tooltip body for an edge. */
export function edgeTooltipHtml(e: StarEdge): string {
    const head = `${esc(e.from)} ${REL_TEXT[e.rel]} ${esc(e.to)}`;
    if (e.weight === undefined) return `${head}<br/>source: ${esc(e.source)}`;
    return `${head}<br/>${e.weight} calls, ${e.errors ?? 0} errors · source: ${esc(e.source)}`;
}
