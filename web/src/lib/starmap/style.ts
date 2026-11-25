// Star Map styling: pure functions from an API node or edge to the ECharts
// item that draws it. Every node is a circle filled with the colour of its
// kind. A lens changes the ring, the mark before the caption and the
// tooltip, never the node set, the fill or the position. Status is always
// double-encoded (ring colour plus ring width, dash or mark) so it
// survives greyscale.

import { STARMAP } from '../chart-theme.js';
import type { chartTokens } from '../echarts.js';
import type { StarEdge, StarKind, StarNode, StarRel } from '../types.js';
import type { Point } from './layout.js';
import { strong, tip } from '../tooltip';
import type { Tip, TipLine } from '../tooltip';

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

/** Diameter per kind, in pixels: room to be found and pointed at. */
const SIZE: Record<StarKind, number> = {
    org: 44,
    entity: 40,
    system: 40,
    component: 30,
    service: 34,
    workload: 30,
    ext: 34,
    def: 30,
    run: 24,
    slo: 26,
    article: 26,
    gap: 28,
    cluster: 48,
};

/**
 * Fill per kind: one colour each, the same under every lens. None is a
 * status colour of either theme, and none is the yellow lost on white.
 */
export const KIND_COLOR: Record<StarKind, string> = STARMAP.kind;

/** What the legend calls a kind. */
export const KIND_NAME: Record<StarKind, string> = {
    org: 'org unit',
    entity: 'ICT entity',
    system: 'system',
    component: 'component',
    service: 'service',
    workload: 'workload',
    ext: 'external',
    def: 'experiment',
    run: 'run',
    slo: 'SLO',
    article: 'article',
    gap: 'gap',
    cluster: 'cluster',
};

/** Kinds that keep their caption when the map is zoomed out. */
const ALWAYS_LABELLED = new Set<StarKind>(['org', 'entity', 'system', 'cluster']);

/** Every relation the API names, in the order the legend lists them. */
export const RELS: StarRel[] = [
    'owns',
    'implements',
    'calls',
    'runs_on',
    'depends_on',
    'tested_by',
    'broke',
    'covers',
    'measures',
];

/** A relation of the API, or the link from a node to its parent. */
export type RelKey = StarRel | 'part_of';

/** Every relation the map draws, in the order the legend lists them. */
export const REL_KEYS: RelKey[] = [...RELS, 'part_of'];

/** How the line of a relation is drawn. */
export interface Stroke {
    color: string;
    width: number;
    /** `solid`, or the lengths of dash and gap in turn. */
    type: 'solid' | number[];
    opacity: number;
}

/**
 * Colour and dash per relation: two channels, so a relation is told from
 * the others also in greyscale. Every colour is a mid tone that stands
 * out on the dark and on the light panel, and none is a status colour.
 */
const REL_LINE: Record<Exclude<RelKey, 'broke' | 'part_of'>, [string, Stroke['type']]> = {
    owns: [STARMAP.relation.owns, [12, 4]],
    implements: [STARMAP.relation.implements, [10, 3, 2, 3]],
    calls: [STARMAP.relation.calls, 'solid'],
    runs_on: [STARMAP.relation.runs_on, [2, 3]],
    depends_on: [STARMAP.relation.depends_on, [12, 3, 5, 3]],
    tested_by: [STARMAP.relation.tested_by, [5, 4]],
    covers: [STARMAP.relation.covers, [10, 3, 2, 3, 2, 3]],
    measures: [STARMAP.relation.measures, [2, 7]],
};

const LINE_WIDTH = 1.6;
const BROKE_WIDTH = 3;

const SOON_NS = 14 * 86_400_000_000_000;
const DIMMED = 0.35;
/** Longest caption under a node, in characters. */
export const CAPTION = 16;

export interface ItemStyle {
    color: string;
    borderColor?: string;
    borderWidth?: number;
    borderType?: string | number[];
    opacity?: number;
    shadowBlur?: number;
    shadowColor?: string;
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
    /** Which relation the link draws. */
    rel: RelKey;
    lineStyle: Stroke & { curveness?: number };
    /** The caption along the edge; the relation as the API names it. */
    label: { show: boolean; formatter?: string; position?: string; distance?: number[] };
    /** On hover the caption is written also where it is left out at rest. */
    emphasis?: { label: { show: boolean } };
    /** The API edge, or null for a spine parent that has no edge. */
    raw: StarEdge | null;
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
    return zoom >= 1 || ALWAYS_LABELLED.has(kind);
}

export type Status = 'ok' | 'warn' | 'fail';

const severe = (worst: string | null) => worst === 'critical' || worst === 'high';

function riskStatus(n: StarNode): Status | null {
    if (n.risks.open <= 0) return null;
    return severe(n.risks.worst) ? 'fail' : 'warn';
}

function conformanceStatus(n: StarNode): Status | null {
    if (n.conformance === 'spine') return 'ok';
    if (n.conformance === 'target') return 'warn';
    if (n.conformance === 'none') return 'fail';
    return null;
}

function evidenceStatus(n: StarNode): Status | null {
    if (n.evidence.failed > 0) return 'fail';
    if (n.evidence.runs + n.evidence.manual > 0) return 'ok';
    return null;
}

/** What the lens says about a node, or null when it says nothing. */
export function statusOf(n: StarNode, lens: Lens): Status | null {
    if (lens === 'risk') return riskStatus(n);
    if (lens === 'conformance') return conformanceStatus(n);
    return evidenceStatus(n);
}

/** The mark before a caption and the ring width: status never by colour alone. */
export const STATUS_MARK: Record<Status, string> = { ok: '\u2713', warn: '!', fail: '\u2715' };
const RING: Record<Status, number> = { ok: 2.5, warn: 3.5, fail: 4.5 };

/**
 * The caption under a node: the mark of its status under the lens, then
 * its name, cut to `limit` characters. The full name is in the tooltip.
 */
export function caption(n: StarNode, lens: Lens, limit = CAPTION): string {
    const status = statusOf(n, lens);
    const name = n.label.length > limit ? `${n.label.slice(0, limit - 1)}\u2026` : n.label;
    const text = labelText({ ...n, label: name });
    return status ? `${STATUS_MARK[status]} ${text}` : text;
}

function ring(n: StarNode, c: StyleContext): Partial<ItemStyle> {
    const status = statusOf(n, c.lens);
    if (!status) return { borderColor: c.tokens.axis, borderWidth: 1 };
    return { borderColor: c.tokens[status], borderWidth: RING[status] };
}

function gapStyle(n: StarNode, t: Tokens): Partial<ItemStyle> {
    if (n.gap === 'missing' || n.kind === 'gap') {
        return {
            color: t.tooltip.backgroundColor,
            borderColor: t.text,
            borderWidth: 2,
            borderType: 'dashed',
        };
    }
    if (n.gap === 'stale') return { borderColor: t.warn, borderWidth: 2.5, borderType: [2, 3] };
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
    return { opacity: DIMMED, borderColor: c.tokens.text, borderWidth: 1.5, borderType: 'dotted' };
}

function selectedStyle(n: StarNode, c: StyleContext): Partial<ItemStyle> {
    if (n.id !== c.selected) return {};
    const accent = c.tokens.accent;
    return { borderColor: accent, borderWidth: 5, shadowBlur: 16, shadowColor: accent };
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
        symbol: 'circle',
        symbolSize: size(n),
        itemStyle: {
            color: KIND_COLOR[n.kind],
            ...ring(n, c),
            ...gapStyle(n, c.tokens),
            ...suppressedStyle(n, c),
            ...selectedStyle(n, c),
        },
        label: { show: n.id === c.selected || labelShown(n.kind, c.zoom) },
        raw: n,
    };
}

/**
 * The line of a relation: what a run broke is red and thick, the link to
 * a parent thin and grey, every other relation has its colour and dash.
 */
export function relStroke(rel: RelKey, t: Tokens): Stroke {
    if (rel === 'part_of') return { color: t.text, width: 1, type: 'solid', opacity: 0.55 };
    if (rel === 'broke') return { color: t.fail, width: BROKE_WIDTH, type: 'solid', opacity: 0.85 };
    const [color, type] = REL_LINE[rel];
    return { color, width: LINE_WIDTH, type, opacity: 0.85 };
}

function lineStyle(e: StarEdge, c: StyleContext): Stroke {
    const stroke = relStroke(e.rel, c.tokens);
    if (e.rel !== 'calls') return stroke;
    // Traffic widens a call, but it stays thinner than what a run broke.
    return { ...stroke, width: stroke.width + Math.min(1, Math.log10(1 + (e.weight ?? 0)) / 3) };
}

/** The relation as it is written along an edge: `RUNS_ON`. */
export function relCaption(rel: StarRel): string {
    return rel.toUpperCase();
}

/** The graph link for one edge: an arrow that carries its relation. */
export function toLink(e: StarEdge, c: StyleContext): StarLink {
    return {
        id: e.id,
        source: e.from,
        target: e.to,
        symbol: ['none', 'arrow'],
        rel: e.rel,
        lineStyle: lineStyle(e, c),
        label: { show: true, formatter: relCaption(e.rel) },
        raw: e,
    };
}

/** The relation written on the link from a node to its parent. */
export const PART_OF = 'PART_OF';

/** The name of a relation on the map and in the legend. */
export function relName(rel: RelKey): string {
    return rel === 'part_of' ? PART_OF : relCaption(rel);
}

/**
 * The link from a child to a parent the API joins by no edge (an org unit
 * under its parent unit, a component in its system). It draws the `parent`
 * of the node: a thin grey arrow, so that it reads as structure.
 */
export function toSpineLink(child: string, parent: string, c: StyleContext): StarLink {
    return {
        id: `spine|${child}`,
        source: child,
        target: parent,
        symbol: ['none', 'arrow'],
        rel: 'part_of',
        lineStyle: relStroke('part_of', c.tokens),
        label: { show: true, formatter: PART_OF },
        raw: null,
    };
}

const day = (ns: number) => new Date(ns / 1_000_000).toISOString().slice(0, 10);

function suppressionLine(n: StarNode): string | null {
    if (n.risks.suppressed <= 0) return null;
    const who = n.risks.suppressed_by ?? 'unknown';
    const until =
        n.risks.suppression_expires_ns === null
            ? 'with no expiry'
            : `until ${day(n.risks.suppression_expires_ns)}`;
    return `suppressed by ${who} ${until}: ${n.risks.suppression_reason ?? ''}`;
}

function riskLine(n: StarNode): string | null {
    if (n.risks.open + n.risks.suppressed <= 0) return null;
    const worst = n.risks.worst ? ` (worst ${n.risks.worst})` : '';
    return `risks: ${n.risks.open} open${worst}, ${n.risks.suppressed} suppressed`;
}

function gapLines(n: StarNode): string[] {
    if (!n.gap) return [];
    const wanted = n.gap === 'missing' ? 'missing' : 'stale';
    const names = Object.entries(n.evidence.by_discipline)
        .filter(([, status]) => status === wanted)
        .map(([discipline]) => discipline);
    const head = n.gap === 'missing' ? 'no evidence' : 'stale evidence';
    const lines = [names.length ? `${head}: ${names.join(', ')}` : head];
    if (n.risks.recommended.length) {
        lines.push(`recommended: ${n.risks.recommended.join(', ')}`);
    }
    return lines;
}

function evidenceLine(n: StarNode): string | null {
    const e = n.evidence;
    if (e.runs + e.manual <= 0) return null;
    const last = e.last_run_ns === null ? '' : `, last ${day(e.last_run_ns)}`;
    return `${e.runs} runs (${e.failed} failed), ${e.manual} manual${last}`;
}

function headLine(n: StarNode): TipLine {
    const kind = n.kind === 'org' && n.org_kind ? n.org_kind : n.kind;
    const env = n.env ? ` · ${n.env}` : '';
    return [strong(n.label), ` (${kind}${env})`];
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
    if (n.tier) lines.push(`tier: ${n.tier}`);
    if (n.posture) lines.push(`resilience: ${n.posture.score.toFixed(1)} (${n.posture.band})`);
    if (n.conflict) lines.push('reports more than one entity id');
    for (const flag of n.flags) lines.push(flagText(flag));
    lines.push(...rollupLines(n));
    if (n.declared) lines.push('declared only, not seen in telemetry');
    if (n.members !== null) lines.push(`${n.members} nodes collapsed; click to expand`);
    return lines;
}

/**
 * Tooltip of a node under the active lens. The lines here are plain
 * text: `tip` escapes every one of them.
 */
export function tooltipHtml(n: StarNode, c: StyleContext): Tip {
    const lines: TipLine[] = [headLine(n), ...detailLines(n)];
    if (c.lens === 'conformance' && n.kind === 'service') {
        lines.push(`conformance: ${n.conformance ?? 'unknown'}`);
    }
    lines.push(evidenceLine(n), ...gapLines(n), riskLine(n), suppressionLine(n));
    return tip(...lines);
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

/** Tooltip of an edge. */
export function edgeTooltipHtml(e: StarEdge): Tip {
    const head = `${e.from} ${REL_TEXT[e.rel]} ${e.to}`;
    if (e.weight === undefined) return tip(head, `source: ${e.source}`);
    return tip(head, `${e.weight} calls, ${e.errors ?? 0} errors · source: ${e.source}`);
}

/** Tooltip of a PART_OF link: which node is part of which, and where that comes from. */
export function spineTooltipHtml(child: string, parent: string): Tip {
    return tip(`${child} part of ${parent}`, 'source: the parent of the node');
}
