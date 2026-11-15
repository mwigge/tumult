// The Star Map as an ECharts graph series, built by pure functions: round
// nodes captioned below, directed edges drawn in the line of their
// relation and named where there is room along them, a PART_OF arrow to
// a parent the API joins by no edge, parallel edges bent apart, and the
// neighbourhood of the selected node kept while the rest is dimmed. The
// page adds the tooltip and hands the series to the chart.

import type { StarEdge, StarKind, StarNode } from '../types.js';
import type { Point } from './layout.js';
import { SPACING } from './force.js';
import {
    CAPTION,
    KINDS,
    KIND_COLOR,
    KIND_NAME,
    REL_KEYS,
    STATUS_MARK,
    caption,
    relName,
    relStroke,
    toItem,
    toLink,
    toSpineLink,
} from './style.js';
import type {
    Lens,
    RelKey,
    StarItem,
    StarLink,
    Status,
    Stroke,
    StyleContext,
    Tokens,
} from './style.js';

/** With more links than this the relation captions wait for a hover. */
export const CAPTION_LIMIT = 150;

/** Room kept free at the sides and at top and bottom of the canvas, in pixels. */
export const VIEW_PAD: Point = [70, 48];

/** The most a map is stretched along one axis to fill the canvas. */
export const STRETCH_LIMIT = 1.6;

/** Width of one character of a relation caption, and the air around it. */
const CAPTION_CHAR = 5.5;
const CAPTION_AIR = 6;
/** Half the height of a relation caption. */
const CAPTION_RISE = 8;
/** How far above its line the middle of a relation caption is. */
const CAPTION_LIFT = 10;
/** The gap the chart leaves between a line and the caption above it. */
const CAPTION_GAP = 5;
/** Shares of the drawn line where a relation is tried, in turn. */
export const CAPTION_SPOTS = [0.5, 0.35, 0.65];
/** Room between a node and its tooltip. */
const TIP_GAP = 8;
/** A node caption: gap below the node, half a character, half a line. */
const NAME_GAP = 4;
const NAME_CHAR = 2.8;
const NAME_RISE = 8;
/** What the small kind line under a name adds to half the height of a caption. */
const KIND_RISE = 5;
/** Room a status mark takes before a name, and the shortest name kept. */
const NAME_MARK = 11;
const NAME_MIN = 6;
/** Two neighbours seldom both have a long name: a name may use this much of the gap. */
const NAME_SHARE = 1.2;

/** How far a parallel edge bends, as a share of its length, per step. */
const BEND = 0.2;
const DIM_NODE = 0.2;
const DIM_LINK = 0.1;
/** The zoom a caption is fitted for when the level of detail is below 1. */
const MIN_ZOOM = 0.5;

export interface StarView {
    nodes: StarNode[];
    edges: StarEdge[];
    positions: Map<string, Point>;
    /** Size of the chart in pixels. */
    canvas: Canvas;
    context: StyleContext;
    /** Kinds switched off in the legend. */
    hidden: ReadonlySet<StarKind>;
    /** Relations switched off in the legend. */
    hiddenRels: ReadonlySet<RelKey>;
    /** Whether the map shows the neighbourhood of one node. */
    focused: boolean;
    light: boolean;
}

export interface Canvas {
    width: number;
    height: number;
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

export interface LegendRel {
    rel: RelKey;
    name: string;
    count: number;
    stroke: Stroke;
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

const round = (v: number) => Math.round(v * 100) / 100;

function span(values: number[]): number {
    return values.length ? Math.max(...values) - Math.min(...values) : 0;
}

/**
 * Pixels per layout unit when the chart fits the map inside the canvas,
 * `VIEW_PAD` away from its sides. 1 when there is nothing to fit.
 */
export function fitScale(positions: Map<string, Point>, canvas: Canvas): number {
    const room = [canvas.width - 2 * VIEW_PAD[0], canvas.height - 2 * VIEW_PAD[1]];
    const points = [...positions.values()];
    const spans = [span(points.map((p) => p[0])), span(points.map((p) => p[1]))];
    const scales = [0, 1].filter((i) => spans[i] > 0 && room[i] > 0).map((i) => room[i] / spans[i]);
    return scales.length ? Math.min(...scales) : 1;
}

/**
 * The map stretched along one axis to the shape of the canvas, by at most
 * `STRETCH_LIMIT`. Nothing moves closer together, and the chart then
 * draws the whole map larger.
 */
export function fillCanvas(positions: Map<string, Point>, canvas: Canvas): Map<string, Point> {
    const room = [canvas.width - 2 * VIEW_PAD[0], canvas.height - 2 * VIEW_PAD[1]];
    const points = [...positions.values()];
    const spans = [span(points.map((p) => p[0])), span(points.map((p) => p[1]))];
    if (spans.some((s) => s <= 0) || room.some((r) => r <= 0)) return positions;
    // Above 1 the map is flatter than the canvas and grows up and down.
    const flat = spans[0] / spans[1] / (room[0] / room[1]);
    const by: Point = [
        Math.min(STRETCH_LIMIT, Math.max(1, 1 / flat)),
        Math.min(STRETCH_LIMIT, Math.max(1, flat)),
    ];
    if (by[0] === 1 && by[1] === 1) return positions;
    const low = [Math.min(...points.map((p) => p[0])), Math.min(...points.map((p) => p[1]))];
    const grown = new Map<string, Point>();
    for (const [id, p] of positions) {
        grown.set(id, [
            round(low[0] + (p[0] - low[0]) * by[0]),
            round(low[1] + (p[1] - low[1]) * by[1]),
        ]);
    }
    return grown;
}

/**
 * Whether `text` has room on the edge from `a` to `b`, between the rims
 * of two nodes `sizeA` and `sizeB` pixels across, drawn at `scale`, when
 * its middle is at the share `at` of the drawn line.
 */
export function captionFits(
    text: string,
    a: Point,
    b: Point,
    sizeA: number,
    sizeB: number,
    scale: number,
    at = 0.5,
): boolean {
    const drawn = Math.hypot(b[0] - a[0], b[1] - a[1]) * scale - (sizeA + sizeB) / 2;
    return round(Math.min(at, 1 - at) * drawn) >= (text.length * CAPTION_CHAR + CAPTION_AIR) / 2;
}

/**
 * A box by its centre and half its width and height. A box `turn`ed is
 * drawn along the unit vector `u`, `half` long and wide; `w` and `h` are
 * then the upright box around it.
 */
export interface Box {
    x: number;
    y: number;
    w: number;
    h: number;
    turn?: { u: Point; half: Point };
}

/**
 * The box of `text` written along the edge from `a` to `b`, with its
 * middle at the share `at` of the drawn line. The edge bends by `bend`
 * of its length. The chart writes the text upright, just above the line,
 * or just `below` it.
 */
export function captionBox(
    text: string,
    a: Point,
    b: Point,
    bend: number,
    sizes: Point = [0, 0],
    at = 0.5,
    below = false,
): Box {
    const [dx, dy] = [b[0] - a[0], b[1] - a[1]];
    const length = Math.hypot(dx, dy) || 1;
    const [ux, uy] = [Math.abs(dx) / length, Math.abs(dy) / length];
    // Unit vector from the line to the text: up, as seen along the edge read left to right.
    const turn = (dx < 0 ? -1 : 1) * (below ? -1 : 1);
    const lift: Point = [(turn * dy * CAPTION_LIFT) / length, (-turn * dx * CAPTION_LIFT) / length];
    const along = (text.length * CAPTION_CHAR) / 2;
    // The line is drawn between the rims: how far from `a` the caption is.
    const from = sizes[0] / 2 + (length - (sizes[0] + sizes[1]) / 2) * at;
    return {
        x: round(a[0] + (dx * from) / length + (dy * bend) / 2 + lift[0]),
        y: round(a[1] + (dy * from) / length - (dx * bend) / 2 + lift[1]),
        w: along * ux + CAPTION_RISE * uy,
        h: along * uy + CAPTION_RISE * ux,
        // The sign of `u` does not matter; `+ 0` keeps -0 out.
        turn: { u: [ux, Math.sign(dx || 1) * (dy / length) + 0], half: [along, CAPTION_RISE] },
    };
}

/** The two axes of a box and how far it reaches along each. */
function sides(box: Box): { axes: [Point, Point]; half: Point } {
    if (!box.turn)
        return {
            axes: [
                [1, 0],
                [0, 1],
            ],
            half: [box.w, box.h],
        };
    const [ux, uy] = box.turn.u;
    return {
        axes: [
            [ux, uy],
            [-uy, ux],
        ],
        half: box.turn.half,
    };
}

/** Whether two boxes, upright or turned, share any area: no axis of either keeps them apart. */
function crosses(a: Box, b: Box): boolean {
    const [sa, sb] = [sides(a), sides(b)];
    const d: Point = [b.x - a.x, b.y - a.y];
    const dot = (p: Point, q: Point) => p[0] * q[0] + p[1] * q[1];
    const reach = (s: typeof sa, axis: Point) =>
        s.half[0] * Math.abs(dot(s.axes[0], axis)) + s.half[1] * Math.abs(dot(s.axes[1], axis));
    return [...sa.axes, ...sb.axes].every(
        (axis) => Math.abs(dot(d, axis)) < reach(sa, axis) + reach(sb, axis) - 1e-9,
    );
}

/** Whether `box` shares any area with one of `others`. */
export function overlaps(box: Box, others: Box[]): boolean {
    return others.some((o) => crosses(box, o));
}

/** An edge as it is drawn, and the relation to write along it. */
export interface DrawnEdge {
    text: string;
    a: Point;
    b: Point;
    /** Diameters of the nodes at `a` and `b`. */
    sizes: Point;
    bend: number;
}

/**
 * Shares of a line `drawn` pixels long where `text` is tried, in turn:
 * `CAPTION_SPOTS`, then the two places where the text ends just short
 * of a rim, at the free end of an edge whose other end is crowded.
 */
export function captionShares(text: string, drawn: number): number[] {
    const half = (text.length * CAPTION_CHAR + CAPTION_AIR) / 2;
    // Rounded up to whole hundredths, so that the text still fits there.
    const flush = Math.ceil(round((half / drawn) * 100)) / 100;
    if (flush >= 0.5 || CAPTION_SPOTS.includes(flush)) return CAPTION_SPOTS;
    return [...CAPTION_SPOTS, flush, round(1 - flush)];
}

/** A place for a relation: the share of the drawn line, and which side of it. */
export interface Spot {
    at: number;
    below: boolean;
}

/**
 * Where the relation is written: the first of `captionShares`, above the
 * line and then below it, where the text has room between the rims and
 * lands on none of `taken`. Null when there is no such place. A bent edge
 * has its middle only.
 */
export function captionSpot(edge: DrawnEdge, taken: Box[]): Spot | null {
    const { text, a, b, sizes, bend } = edge;
    const drawn = Math.hypot(b[0] - a[0], b[1] - a[1]) - (sizes[0] + sizes[1]) / 2;
    const shares = bend === 0 ? captionShares(text, drawn) : [0.5];
    for (const at of shares) {
        if (!captionFits(text, a, b, sizes[0], sizes[1], 1, at)) continue;
        for (const below of [false, true]) {
            if (!overlaps(captionBox(text, a, b, bend, sizes, at, below), taken)) {
                return { at, below };
            }
        }
    }
    return null;
}

type Side = 'Top' | 'Bottom';

/** Where the chart writes an edge label. */
export interface Anchor {
    position: 'middle' | `inside${'Start' | 'Middle' | 'End'}${Side}`;
    /** Pixels from the end of the line to the text, and from the line to it. */
    distance?: [number, number];
}

/** A line as the chart draws it: from the centre of its source to the rim of its target. */
export interface DrawnLine {
    /** Length between the two rims. */
    drawn: number;
    /** Half the source node: the part of the line inside it. */
    rim: number;
    bent: boolean;
    /** Pixels per unit of the map: the chart reads a distance in those units. */
    scale: number;
}

/**
 * How the chart is told to write a caption at `spot`: measured from the
 * start of the line or from its end, to the middle of the text. On a bent
 * edge the chart writes it in the middle of the curve.
 */
export function captionAnchor(spot: Spot, line: DrawnLine): Anchor {
    const side: Side = spot.below ? 'Bottom' : 'Top';
    if (line.bent) {
        if (!spot.below) return { position: 'middle' };
        return { position: 'insideMiddleBottom', distance: [0, round(CAPTION_GAP * line.scale)] };
    }
    const gap = round(CAPTION_GAP * line.scale);
    if (spot.at > 0.5) {
        const fromEnd = round((1 - spot.at) * line.drawn * line.scale);
        return { position: `insideEnd${side}`, distance: [fromEnd, gap] };
    }
    const fromStart = round((line.rim + spot.at * line.drawn) * line.scale);
    return { position: `insideStart${side}`, distance: [fromStart, gap] };
}

/** A rectangle of the chart by its top left corner. */
export interface Rect {
    x: number;
    y: number;
    width: number;
    height: number;
}

/**
 * Where the tooltip of a node goes: above the node, so that the caption
 * under it stays in sight; beside the node and its caption when there is
 * no room above. `content` and `view` are the sizes of tooltip and chart.
 */
export function tipPlace(node: Rect, content: Point, view: Point): Point {
    const centre = node.x + node.width / 2;
    const top = node.y - content[1] - TIP_GAP;
    if (top >= 0) {
        return [Math.max(0, Math.min(view[0] - content[0], centre - content[0] / 2)), top];
    }
    const reach = Math.max(node.width / 2, CAPTION * NAME_CHAR + 2 * NAME_GAP) + TIP_GAP;
    const right = centre + reach;
    return [right + content[0] <= view[0] ? right : centre - reach - content[0], 0];
}

export interface Pair {
    source: string;
    target: string;
}

/** Index of every link that starts or ends at node `id`, in order. */
export function linksAt(links: Pair[], id: string): number[] {
    const at: number[] = [];
    links.forEach((link, i) => {
        if (link.source === id || link.target === id) at.push(i);
    });
    return at;
}

/** Ids of the nodes that have the same name as a node one link away. */
export function twins(nodes: StarNode[], pairs: Pair[]): Set<string> {
    const name = new Map(nodes.map((n) => [n.id, n.label.toLowerCase()]));
    const same = new Set<string>();
    for (const { source, target } of pairs) {
        if (source === target || !name.has(source)) continue;
        if (name.get(source) !== name.get(target)) continue;
        same.add(source).add(target);
    }
    return same;
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

/** The relations drawn between `nodes`, with their line, for the legend chips. */
export function legendRels(nodes: StarNode[], edges: StarEdge[], tokens: Tokens): LegendRel[] {
    const ids = new Set(nodes.map((n) => n.id));
    const counts = new Map<RelKey, number>();
    for (const e of edges) {
        if (!ids.has(e.from) || !ids.has(e.to)) continue;
        counts.set(e.rel, (counts.get(e.rel) ?? 0) + 1);
    }
    const parents = spinePairs(nodes, edges).length;
    if (parents > 0) counts.set('part_of', parents);
    return REL_KEYS.filter((rel) => counts.has(rel)).map((rel) => ({
        rel,
        name: relName(rel),
        count: counts.get(rel) ?? 0,
        stroke: relStroke(rel, tokens),
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
    return links.filter(
        (l) => ids.has(l.source) && ids.has(l.target) && !view.hiddenRels.has(l.rel),
    );
}

/** Where the map is drawn: positions in pixels, and what is already written there. */
interface Drawn {
    at: (id: string) => Point;
    size: Map<string, number>;
    /** Box of each node caption, by node id. */
    names: Map<string, Box>;
    /** Box of each node disc, by node id. */
    discs: Map<string, Box>;
    /** Pixels per unit of the map. */
    scale: number;
}

function drawnMap(view: StarView, data: StarItem[], nameOf: (n: StarNode) => string): Drawn {
    const scale = fitScale(view.positions, view.canvas) * Math.max(view.context.zoom, MIN_ZOOM);
    const at = (id: string): Point => {
        const p = view.positions.get(id) ?? [0, 0];
        return [p[0] * scale, p[1] * scale];
    };
    const names = new Map<string, Box>();
    const discs = new Map<string, Box>();
    for (const item of data) {
        const [x, y] = at(item.id);
        const half = item.symbolSize / 2;
        discs.set(item.id, { x, y, w: half, h: half });
        if (!item.label.show) continue;
        const lines = nameOf(item.raw).split('\n');
        const h = NAME_RISE + KIND_RISE * (lines.length - 1);
        const w = Math.max(...lines.map((l) => l.length)) * NAME_CHAR + NAME_GAP;
        names.set(item.id, { x, y: y + half + NAME_GAP + h, w, h });
    }
    const size = new Map(data.map((item) => [item.id, item.symbolSize]));
    return { at, size, names, discs, scale };
}

/** Where a relation that must be written goes: clear of the names already written, if it can. */
function forcedSpot(edge: DrawnEdge, written: Box[]): Spot {
    return captionSpot(edge, written) ?? { at: 0.5, below: false };
}

/**
 * Says for each link, in turn, where its relation can be written: the
 * edge is long enough, and the caption lands on no node, no node caption
 * and no relation written before it. A `forced` relation that has no
 * such place is written clear of the other relations only.
 */
function captionRoom(map: Drawn, bend: Map<string, number>) {
    const written: Box[] = [];
    return (link: StarLink, forced: boolean): Anchor | null => {
        const [a, b] = [map.at(link.source), map.at(link.target)];
        const sizes: Point = [map.size.get(link.source) ?? 0, map.size.get(link.target) ?? 0];
        const edge = { text: relName(link.rel), a, b, sizes, bend: bend.get(link.id) ?? 0 };
        const others = [...map.discs].filter(([id]) => id !== link.source && id !== link.target);
        const taken = [...map.names.values(), ...others.map(([, disc]) => disc), ...written];
        const spot = captionSpot(edge, taken) ?? (forced ? forcedSpot(edge, written) : null);
        if (spot === null) return null;
        written.push(captionBox(edge.text, a, b, edge.bend, sizes, spot.at, spot.below));
        const drawn = Math.hypot(b[0] - a[0], b[1] - a[1]) - (sizes[0] + sizes[1]) / 2;
        return captionAnchor(spot, {
            drawn,
            rim: sizes[0] / 2,
            bent: edge.bend !== 0,
            scale: map.scale,
        });
    };
}

/**
 * Where each relation is written, by link id. The links of the selected
 * node come first and are always written: the rest of the map is dimmed
 * then, so nothing is in their way.
 */
function anchors(view: StarView, links: StarLink[], bend: Map<string, number>, map: Drawn) {
    const captions = captionsShown(links.length, view.context.zoom, view.focused);
    const room = captionRoom(map, bend);
    const selected = view.context.selected;
    const own = (link: StarLink) => link.source === selected || link.target === selected;
    const placed = new Map<string, Anchor>();
    // Then the API edges, and the PART_OF links in what room is left.
    const rank = (link: StarLink) => (own(link) ? 0 : link.raw ? 1 : 2);
    for (const link of [...links].sort((a, b) => rank(a) - rank(b))) {
        if (!captions && !own(link)) continue;
        const anchor = room(link, own(link));
        if (anchor) placed.set(link.id, anchor);
    }
    return placed;
}

/** Bend, caption and dim the links for the zoom and the selection. */
function finishLinks(view: StarView, links: StarLink[], map: Drawn, near: Set<string> | null) {
    const bend = bends(links);
    const placed = anchors(view, links, bend, map);
    const selected = view.context.selected;
    return links.map((link) => {
        const own = link.source === selected || link.target === selected;
        const anchor = placed.get(link.id);
        const drawn: StarLink = {
            ...link,
            lineStyle: { ...link.lineStyle, curveness: bend.get(link.id) ?? 0 },
            label: { ...link.label, show: anchor !== undefined, ...anchor },
            emphasis: { label: { show: true } },
        };
        return near !== null && !own ? dimLink(drawn) : drawn;
    });
}

interface LabelParams {
    data?: { raw?: StarNode };
}

/**
 * The characters of a name that fit between two nodes side by side when
 * the map is drawn at `scale` pixels per layout unit.
 */
export function nameLimit(scale: number): number {
    const room = SPACING * scale * NAME_SHARE - 2 * NAME_GAP - NAME_MARK;
    const fit = Math.floor(room / (2 * NAME_CHAR));
    return Math.min(CAPTION, Math.max(NAME_MIN, fit));
}

/** The graph series of the Star Map. */
export function starSeries(raw: StarView) {
    const view = { ...raw, positions: fillCanvas(raw.positions, raw.canvas) };
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
    const same = twins(shown, links);
    const scale = fitScale(view.positions, view.canvas) * Math.max(view.context.zoom, MIN_ZOOM);
    const limit = nameLimit(scale);
    const nameOf = (n: StarNode): string => {
        const name = caption(n, view.context.lens, limit);
        return same.has(n.id) ? `${name}\n{kind|${KIND_NAME[n.kind]}}` : name;
    };
    return {
        id: 'star',
        type: 'graph' as const,
        layout: 'none' as const,
        // The map keeps its shape inside the padded canvas, as `fitScale` says.
        left: VIEW_PAD[0],
        right: VIEW_PAD[0],
        top: VIEW_PAD[1],
        bottom: VIEW_PAD[1],
        preserveAspect: 'contain' as const,
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
            // A pill in the panel colour: a line under the caption stays out of it,
            // also when the node itself is faded.
            opacity: 1,
            backgroundColor: t.tooltip.backgroundColor,
            padding: [1, 4],
            borderRadius: 3,
            rich: { kind: { fontSize: 9, color: t.text, align: 'center' as const } },
            formatter: (p: LabelParams): string => (p.data?.raw ? nameOf(p.data.raw) : ''),
        },
        edgeLabel: {
            show: false,
            color: t.text,
            fontSize: 9,
            fontFamily: 'monospace',
            align: 'center' as const,
            ...halo,
        },
        labelLayout: { hideOverlap: true },
        categories: KINDS.map((k) => ({ name: k })),
        lineStyle: { color: t.text, opacity: 0.85 },
        data,
        links: finishLinks(view, links, drawnMap(view, data, nameOf), near),
        animation: false,
    };
}
