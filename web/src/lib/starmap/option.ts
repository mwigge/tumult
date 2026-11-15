// The Star Map as an ECharts graph series, built by pure functions: round
// nodes captioned below, directed edges that carry their relation, a plain
// line for a spine parent the API joins by no edge, parallel edges bent
// apart, and the neighbourhood of the selected node kept while the rest
// is dimmed. The page adds the tooltip and hands the series to the chart.

import type { StarEdge, StarKind, StarNode } from '../types.js';
import type { Point } from './layout.js';
import {
    KINDS,
    KIND_COLOR,
    KIND_NAME,
    STATUS_MARK,
    caption,
    toItem,
    toLink,
    toSpineLink,
} from './style.js';
import type { Lens, StarItem, StarLink, Status, StyleContext } from './style.js';

/** With more edges than this the relation captions wait for a hover. */
export const CAPTION_LIMIT = 60;

/** How far a parallel edge bends, as a share of its length, per step. */
const BEND = 0.2;
const DIM_NODE = 0.2;
const DIM_LINK = 0.1;

export interface StarView {
    nodes: StarNode[];
    edges: StarEdge[];
    positions: Map<string, Point>;
    context: StyleContext;
    /** Kinds switched off in the legend. */
    hidden: ReadonlySet<StarKind>;
    /** Whether the map shows the neighbourhood of one node. */
    focused: boolean;
    light: boolean;
}

export interface SpinePair {
    child: string;
    parent: string;
}

export interface LegendKind {
    kind: StarKind;
    name: string;
    color: string;
    count: number;
}

const pairKey = (a: string, b: string) => (a < b ? `${a}\u0000${b}` : `${b}\u0000${a}`);

/**
 * Parents that are on the map and joined to their child by no edge, in
 * either direction, sorted by child. Drawn so that no node floats free.
 */
export function spinePairs(nodes: StarNode[], edges: StarEdge[]): SpinePair[] {
    const ids = new Set(nodes.map((n) => n.id));
    const joined = new Set(edges.map((e) => pairKey(e.from, e.to)));
    const pairs: SpinePair[] = [];
    for (const n of nodes) {
        if (!n.parent || n.parent === n.id || !ids.has(n.parent)) continue;
        if (joined.has(pairKey(n.id, n.parent))) continue;
        pairs.push({ child: n.id, parent: n.parent });
    }
    return pairs.sort((a, b) => (a.child < b.child ? -1 : 1));
}

/** Whether relation captions are written without a hover. */
export function captionsShown(edges: number, zoom: number, focused: boolean): boolean {
    if (focused) return true;
    return zoom >= 1 && edges <= CAPTION_LIMIT;
}

/** The kinds on the map, centre first, for the legend chips. */
export function legendKinds(nodes: StarNode[]): LegendKind[] {
    const counts = new Map<StarKind, number>();
    for (const n of nodes) counts.set(n.kind, (counts.get(n.kind) ?? 0) + 1);
    return KINDS.filter((kind) => counts.has(kind)).map((kind) => ({
        kind,
        name: KIND_NAME[kind],
        color: KIND_COLOR[kind],
        count: counts.get(kind) ?? 0,
    }));
}

export interface StatusKey {
    status: Status;
    mark: string;
    text: string;
}

const STATUS_TEXT: Record<Lens, Partial<Record<Status, string>>> = {
    evidence: { ok: 'runs passed', fail: 'a run failed' },
    risk: { warn: 'open risk', fail: 'high or critical risk' },
    conformance: { ok: 'full telemetry', warn: 'partial telemetry', fail: 'no telemetry tags' },
};

/** What each ring means under a lens, for the legend. */
export function statusKey(lens: Lens): StatusKey[] {
    const order: Status[] = ['ok', 'warn', 'fail'];
    const keys: StatusKey[] = [];
    for (const status of order) {
        const text = STATUS_TEXT[lens][status];
        if (text) keys.push({ status, mark: STATUS_MARK[status], text });
    }
    return keys;
}

/**
 * Bend per link id. A pair joined once gets a straight line. Links that
 * share both ends alternate sides and step outwards; one that runs
 * against the first of its pair takes the opposite sign, which draws it
 * on the side it was given.
 */
function bends(links: StarLink[]): Map<string, number> {
    const groups = new Map<string, StarLink[]>();
    for (const link of links) {
        const key = pairKey(link.source, link.target);
        groups.set(key, [...(groups.get(key) ?? []), link]);
    }
    const bend = new Map<string, number>();
    for (const group of groups.values()) {
        if (group.length === 1) {
            bend.set(group[0].id, 0);
            continue;
        }
        const sorted = [...group].sort((a, b) => (a.id < b.id ? -1 : 1));
        sorted.forEach((link, slot) => {
            const side = slot % 2 === 0 ? 1 : -1;
            const along = link.source === sorted[0].source ? 1 : -1;
            bend.set(link.id, side * along * BEND * (Math.floor(slot / 2) + 1));
        });
    }
    return bend;
}

/** The selected node and everything one link away, or null without one. */
function neighbourhood(selected: string | null, links: StarLink[]): Set<string> | null {
    if (selected === null) return null;
    const near = new Set([selected]);
    for (const link of links) {
        if (link.source === selected) near.add(link.target);
        if (link.target === selected) near.add(link.source);
    }
    return near;
}

function dimItem(item: StarItem): StarItem {
    return {
        ...item,
        itemStyle: { ...item.itemStyle, opacity: DIM_NODE },
        label: { show: false },
    };
}

function dimLink(link: StarLink): StarLink {
    return {
        ...link,
        lineStyle: { ...link.lineStyle, opacity: DIM_LINK },
        label: { ...link.label, show: false },
    };
}

function visibleNodes(view: StarView): StarNode[] {
    return view.nodes.filter((n) => !view.hidden.has(n.kind));
}

function allLinks(view: StarView, shown: StarNode[]): StarLink[] {
    const ids = new Set(shown.map((n) => n.id));
    const links = view.edges.map((e) => toLink(e, view.context));
    for (const pair of spinePairs(shown, view.edges)) {
        links.push(toSpineLink(pair.child, pair.parent, view.context));
    }
    return links.filter((l) => ids.has(l.source) && ids.has(l.target));
}

/** Bend, caption and dim the links for the zoom and the selection. */
function finishLinks(view: StarView, links: StarLink[], near: Set<string> | null): StarLink[] {
    const bend = bends(links);
    const captions = captionsShown(view.edges.length, view.context.zoom, view.focused);
    const selected = view.context.selected;
    return links.map((link) => {
        const own = link.source === selected || link.target === selected;
        const drawn: StarLink = {
            ...link,
            lineStyle: { ...link.lineStyle, curveness: bend.get(link.id) ?? 0 },
            label: { ...link.label, show: link.raw !== null && (captions || own) },
        };
        return near && !own ? dimLink(drawn) : drawn;
    });
}

interface LabelParams {
    data?: { raw?: StarNode };
}

/** The graph series of the Star Map. */
export function starSeries(view: StarView) {
    const t = view.context.tokens;
    const halo = { textBorderColor: t.tooltip.backgroundColor, textBorderWidth: 3 };
    const shown = visibleNodes(view);
    const links = allLinks(view, shown);
    const onMap = shown.some((n) => n.id === view.context.selected);
    const near = onMap && !view.focused ? neighbourhood(view.context.selected, links) : null;
    const data = shown.map((n) => {
        const item = toItem(n, view.positions.get(n.id) ?? [0, 0], view.context);
        return near && !near.has(n.id) ? dimItem(item) : item;
    });
    return {
        id: 'star',
        type: 'graph' as const,
        layout: 'none' as const,
        roam: true,
        scaleLimit: { min: 0.2, max: 8 },
        nodeScaleRatio: 0.4,
        edgeSymbolSize: 9,
        emphasis: {
            focus: 'adjacency' as const,
            label: { show: true },
            edgeLabel: { show: true },
            lineStyle: { width: 2.5, opacity: 1 },
        },
        blur: {
            itemStyle: { opacity: 0.12 },
            lineStyle: { opacity: view.light ? 0.12 : 0.05 },
            label: { show: false },
            edgeLabel: { show: false },
        },
        label: {
            show: true,
            position: 'bottom' as const,
            distance: 4,
            color: t.text,
            fontSize: 11,
            ...halo,
            formatter: (p: LabelParams): string => {
                const raw = p.data?.raw;
                return raw ? caption(raw, view.context.lens) : '';
            },
        },
        edgeLabel: {
            show: false,
            color: t.text,
            fontSize: 9,
            fontFamily: 'monospace',
            ...halo,
        },
        labelLayout: { hideOverlap: true },
        categories: KINDS.map((k) => ({ name: k })),
        lineStyle: { color: t.text, opacity: 0.85 },
        data,
        links: finishLinks(view, links, near),
        animation: false,
    };
}
