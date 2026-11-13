// Radial "star" layout for the Star Map: a pure function from the node
// list (each node names its spine parent) to fixed positions, so the map
// looks the same on every load and tests can assert on it.
//
// The organisation sits in the centre. Each spine node gets a slice of
// the circle proportional to the number of leaves below it and sits in
// the middle of that slice, on the ring of its kind. Nodes off the spine
// (definitions, runs, SLOs, gaps, articles) go on outer rings, turned
// toward the nodes they connect to.

export interface LayoutNode {
    id: string;
    kind: string;
    parent: string | null;
}

export interface LayoutEdge {
    from: string;
    to: string;
}

export type Point = [number, number];
export type Span = [number, number];

/** No two nodes are placed closer than this, in layout units. */
export const MIN_DISTANCE = 28;

/** Smallest distance between two neighbouring rings. */
const RING_STEP = 120;
/** Slack over `MIN_DISTANCE` so rounding never breaks the guarantee. */
const PAD = 1.02;
const TURN = 2 * Math.PI;
/** Slices start at twelve o'clock and run clockwise. */
const START = -Math.PI / 2;

/** Ring of each spine kind; org units share ring 1, split by depth. */
const RING: Record<string, number> = {
    entity: 2,
    cluster: 3,
    system: 3,
    component: 3.5,
    service: 4,
    workload: 5,
    ext: 6,
};
const DEFAULT_RING = 4;
const SATELLITE_RING = 7;
const SATELLITE_KINDS = new Set(['def', 'run', 'slo', 'gap', 'article']);
const OUTER_KIND = 'article';

interface Tree {
    /** The node drawn at the centre, when the set has one organisation root. */
    centre: string | null;
    /** Children per parent id, sorted; the key `''` holds the top level. */
    children: Map<string, string[]>;
    parent: Map<string, string>;
    kind: Map<string, string>;
    satellites: string[];
}

const TOP = '';

/** The spine parent of `n`, unless it is missing or closes a cycle. */
function spineParent(n: LayoutNode, byId: Map<string, LayoutNode>): string | null {
    if (!n.parent || !byId.has(n.parent)) return null;
    let smallest = n.id;
    let at = byId.get(n.parent);
    for (let steps = 0; at && steps <= byId.size; steps += 1) {
        if (at.id === n.id) return smallest === n.id ? null : n.parent;
        if (at.id < smallest) smallest = at.id;
        at = at.parent ? byId.get(at.parent) : undefined;
    }
    return n.parent;
}

function buildTree(nodes: LayoutNode[]): Tree {
    const byId = new Map(nodes.map((n) => [n.id, n]));
    const ids = [...byId.keys()].sort();
    const tree: Tree = {
        centre: null,
        children: new Map([[TOP, []]]),
        parent: new Map(),
        kind: new Map(ids.map((id) => [id, byId.get(id)!.kind])),
        satellites: [],
    };
    const loose: string[] = [];
    for (const id of ids) {
        const n = byId.get(id)!;
        const parent = spineParent(n, byId);
        if (parent === null) {
            loose.push(id);
            continue;
        }
        tree.parent.set(id, parent);
        tree.children.set(parent, [...(tree.children.get(parent) ?? []), id]);
    }
    const companies = loose.filter((id) => tree.kind.get(id) === 'org');
    tree.centre = companies.length === 1 ? companies[0] : null;
    for (const id of loose) {
        if (id === tree.centre) continue;
        if (SATELLITE_KINDS.has(tree.kind.get(id)!)) {
            tree.satellites.push(id);
            continue;
        }
        tree.children.get(TOP)!.push(id);
    }
    return tree;
}

/** Children of the layout root: the centre's own plus the loose nodes. */
function topLevel(tree: Tree): string[] {
    const own = tree.centre ? (tree.children.get(tree.centre) ?? []) : [];
    return [...own, ...tree.children.get(TOP)!].sort();
}

function childrenOf(tree: Tree, id: string): string[] {
    if (id === TOP || id === tree.centre) return topLevel(tree);
    return tree.children.get(id) ?? [];
}

function countLeaves(tree: Tree, id: string, leaves: Map<string, number>): number {
    const below = childrenOf(tree, id);
    const count = below.reduce((sum, child) => sum + countLeaves(tree, child, leaves), 0);
    leaves.set(id, Math.max(1, count));
    return leaves.get(id)!;
}

function assignSpans(tree: Tree): Map<string, Span> {
    const leaves = new Map<string, number>();
    countLeaves(tree, TOP, leaves);
    const spans = new Map<string, Span>([[TOP, [START, START + TURN]]]);
    const queue = [TOP];
    for (let i = 0; i < queue.length; i += 1) {
        const id = queue[i];
        const [start, end] = spans.get(id)!;
        const total = leaves.get(id)!;
        let used = 0;
        for (const child of childrenOf(tree, id)) {
            const from = start + ((end - start) * used) / total;
            used += leaves.get(child)!;
            spans.set(child, [from, start + ((end - start) * used) / total]);
            queue.push(child);
        }
    }
    spans.delete(TOP);
    if (tree.centre) spans.set(tree.centre, [START, START + TURN]);
    return spans;
}

/**
 * The slice of the circle each spine node owns, as `[start, end]` in
 * radians. Satellites have no slice and are absent.
 */
export function spineSpans(nodes: LayoutNode[]): Map<string, Span> {
    return assignSpans(buildTree(nodes));
}

/** Number of org units on the path from the centre down to `id`. */
function orgDepth(tree: Tree, id: string): number {
    let depth = 0;
    let at: string | undefined = id;
    while (at !== undefined && at !== tree.centre) {
        depth += 1;
        at = tree.parent.get(at);
    }
    return depth;
}

/** Ring number per spine node; a child always sits outside its parent. */
function assignRings(tree: Tree, order: string[]): Map<string, number> {
    const orgs = order.filter((id) => tree.kind.get(id) === 'org');
    const deepest = Math.max(1, ...orgs.map((id) => orgDepth(tree, id)));
    const rings = new Map<string, number>();
    for (const id of order) {
        const kind = tree.kind.get(id)!;
        const above = rings.get(tree.parent.get(id) ?? TOP) ?? 0;
        const own = kind === 'org' ? orgDepth(tree, id) / deepest : (RING[kind] ?? DEFAULT_RING);
        rings.set(id, own > above ? own : above + 0.5);
    }
    return rings;
}

/** Spine nodes parent-first, so a ring can build on the parent's ring. */
function parentFirst(tree: Tree): string[] {
    const order = [...topLevel(tree)];
    for (let i = 0; i < order.length; i += 1) {
        order.push(...(tree.children.get(order[i]) ?? []));
    }
    return order;
}

const ringKey = (ring: number) => ring.toFixed(4);

/** Smallest gap between two neighbouring angles on one ring. */
function tightestGap(angles: number[]): number {
    if (angles.length < 2) return TURN;
    const sorted = [...angles].sort((a, b) => a - b);
    let gap = sorted[0] + TURN - sorted[sorted.length - 1];
    for (let i = 1; i < sorted.length; i += 1) {
        gap = Math.min(gap, sorted[i] - sorted[i - 1]);
    }
    return gap;
}

/**
 * Distance between two unit rings that keeps every pair of spine nodes
 * `MIN_DISTANCE` apart: along each ring and from ring to ring.
 */
function ringStep(rings: Map<string, number>, angles: Map<string, number>): number {
    const perRing = new Map<string, { ring: number; angles: number[] }>();
    for (const [id, ring] of rings) {
        const entry = perRing.get(ringKey(ring)) ?? { ring, angles: [] };
        entry.angles.push(angles.get(id)!);
        perRing.set(ringKey(ring), entry);
    }
    const need = MIN_DISTANCE * PAD;
    let step = RING_STEP;
    const levels = [0, ...[...perRing.values()].map((r) => r.ring)].sort((a, b) => a - b);
    for (let i = 1; i < levels.length; i += 1) {
        const apart = levels[i] - levels[i - 1];
        if (apart > 1e-9) step = Math.max(step, need / apart);
    }
    for (const { ring, angles: onRing } of perRing.values()) {
        const half = Math.min(tightestGap(onRing), Math.PI) / 2;
        step = Math.max(step, need / (2 * ring * Math.sin(half)));
    }
    return step;
}

const round = (v: number) => Math.round(v * 100) / 100;

const polar = (radius: number, angle: number): Point => [
    round(radius * Math.cos(angle)),
    round(radius * Math.sin(angle)),
];

function neighbours(edges: LayoutEdge[]): Map<string, Set<string>> {
    const near = new Map<string, Set<string>>();
    const add = (a: string, b: string) => near.set(a, (near.get(a) ?? new Set()).add(b));
    for (const e of edges) {
        add(e.from, e.to);
        add(e.to, e.from);
    }
    return near;
}

/** Mean direction of the placed neighbours of `id`, or null without any. */
function anchor(id: string, near: Map<string, Set<string>>, placed: Map<string, Point>) {
    let x = 0;
    let y = 0;
    for (const other of [...(near.get(id) ?? [])].sort()) {
        const at = placed.get(other);
        if (!at) continue;
        const length = Math.hypot(at[0], at[1]);
        if (length === 0) continue;
        x += at[0] / length;
        y += at[1] / length;
    }
    if (Math.hypot(x, y) < 1e-9) return null;
    const angle = Math.atan2(y, x);
    return angle < START ? angle + TURN : angle;
}

/**
 * Put `ids` on one outer ring, each as close to its anchor as the minimum
 * distance allows. Returns the radius used, which grows when the ring is
 * too short for its nodes.
 */
function placeRing(
    ids: string[],
    radius: number,
    near: Map<string, Set<string>>,
    placed: Map<string, Point>,
): number {
    if (ids.length === 0) return radius;
    const need = MIN_DISTANCE * PAD;
    const wanted = ids
        .map((id) => ({ id, at: anchor(id, near, placed) }))
        .sort((a, b) => (a.at ?? Infinity) - (b.at ?? Infinity) || (a.id < b.id ? -1 : 1));
    const fits = need / (2 * Math.sin(Math.PI / Math.max(2, ids.length)));
    const r = Math.max(radius, ids.length > 1 ? fits : radius);
    const apart = 2 * Math.asin(Math.min(1, need / (2 * r)));
    const angles: number[] = [];
    for (const { at } of wanted) {
        const previous = angles.length ? angles[angles.length - 1] + apart : (at ?? START);
        angles.push(Math.max(at ?? previous, previous));
    }
    const crowded = angles[angles.length - 1] - angles[0] > TURN - apart;
    wanted.forEach(({ id }, i) => {
        const angle = crowded ? angles[0] + (TURN * i) / ids.length : angles[i];
        placed.set(id, polar(r, angle));
    });
    return r;
}

/**
 * Positions for every node. `nodes[].parent` is the spine; `edges` only
 * turn satellites toward what they connect to. The result does not depend
 * on the order of either list.
 */
export function layoutStar(nodes: LayoutNode[], edges: LayoutEdge[] = []): Map<string, Point> {
    const placed = new Map<string, Point>();
    if (nodes.length === 0) return placed;
    const tree = buildTree(nodes);
    const spans = assignSpans(tree);
    const order = parentFirst(tree);
    const rings = assignRings(tree, order);
    const angles = new Map(order.map((id) => [id, (spans.get(id)![0] + spans.get(id)![1]) / 2]));
    const step = ringStep(rings, angles);
    if (tree.centre) placed.set(tree.centre, [0, 0]);
    for (const id of order) {
        placed.set(id, polar(rings.get(id)! * step, angles.get(id)!));
    }
    const near = neighbours(edges);
    const outermost = Math.max(SATELLITE_RING - 1, ...rings.values());
    const inner = tree.satellites.filter((id) => tree.kind.get(id) !== OUTER_KIND);
    const outer = tree.satellites.filter((id) => tree.kind.get(id) === OUTER_KIND);
    const used = placeRing(inner, (outermost + 1) * step, near, placed);
    placeRing(outer, used + step, near, placed);
    return placed;
}
