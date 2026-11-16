import { describe, expect, it } from 'vitest';
import { FORCE_LIMIT, LINK, RISE, SPACING, spread } from './force.js';
import { layoutStar } from './layout.js';
import type { LayoutEdge, LayoutNode, Point } from './layout.js';

const node = (id: string, kind: string, parent: string | null = null): LayoutNode => ({
    id,
    kind,
    parent,
});

function estate(): { nodes: LayoutNode[]; edges: LayoutEdge[] } {
    const nodes = [
        node('org:', 'org'),
        node('org:platform', 'org', 'org:'),
        node('org:platform/edge-team', 'org', 'org:platform'),
        node('org:platform/data-team', 'org', 'org:platform'),
        node('ent:cif-0001', 'entity', 'org:platform/edge-team'),
        node('ent:cif-0002', 'entity', 'org:platform/data-team'),
        node('sys:demo/api-worker', 'system', 'ent:cif-0001'),
        node('sys:demo/message-queue', 'system', 'ent:cif-0002'),
        node('cmp:demo/message-queue/broker', 'component', 'sys:demo/message-queue'),
        node('wl:demo/statefulset/message-queue', 'workload', 'sys:demo/message-queue'),
        node('def:queue-dependency-loss', 'def'),
        node('def:api-worker-kill', 'def'),
        node('def:never-ran', 'def'),
        node('def:orphan', 'def'),
        node('gap:network::partition', 'gap'),
    ];
    const edges = [
        { from: 'sys:demo/message-queue', to: 'def:queue-dependency-loss' },
        { from: 'def:queue-dependency-loss', to: 'sys:demo/message-queue' },
        { from: 'sys:demo/api-worker', to: 'def:api-worker-kill' },
    ];
    return { nodes, edges };
}

const ISOLATED = ['def:never-ran', 'def:orphan', 'gap:network::partition'];

function place(nodes: LayoutNode[], edges: LayoutEdge[]): Map<string, Point> {
    return spread(nodes, edges, layoutStar(nodes, edges));
}

const at = (placed: Map<string, Point>, id: string): Point => {
    const point = placed.get(id);
    if (!point) throw new Error(`${id} is not placed`);
    return point;
};

const distance = (a: Point, b: Point) => Math.hypot(a[0] - b[0], a[1] - b[1]);

/** 1 is the room a node and its caption need: `SPACING` wide, `RISE` high. */
const clearance = (a: Point, b: Point) => Math.hypot((a[0] - b[0]) / SPACING, (a[1] - b[1]) / RISE);

function closest(placed: Map<string, Point>): number {
    const points = [...placed.values()];
    let least = Infinity;
    for (let i = 0; i < points.length; i += 1) {
        for (let j = i + 1; j < points.length; j += 1) {
            least = Math.min(least, clearance(points[i], points[j]));
        }
    }
    return least;
}

/** Length of every link: edges and spine parents. */
function linkLengths(nodes: LayoutNode[], edges: LayoutEdge[], placed: Map<string, Point>) {
    const ids = new Set(nodes.map((n) => n.id));
    const ends = edges.map((e): [string, string] => [e.from, e.to]);
    for (const n of nodes) if (n.parent && ids.has(n.parent)) ends.push([n.id, n.parent]);
    return ends.map(([a, b]) => distance(at(placed, a), at(placed, b)));
}

/** A tree of `count` nodes with three children each, plus a few cross links. */
function tree(count: number): { nodes: LayoutNode[]; edges: LayoutEdge[] } {
    const id = (i: number) => `sys:prod/s-${String(i).padStart(4, '0')}`;
    const nodes = [node('org:', 'org')];
    for (let i = 1; i < count; i += 1) {
        nodes.push(node(id(i), 'system', i < 4 ? 'org:' : id(Math.floor(i / 3))));
    }
    const edges: LayoutEdge[] = [];
    for (let i = 5; i + 7 < count; i += 11) edges.push({ from: id(i), to: id(i + 7) });
    return { nodes, edges };
}

describe('spread', () => {
    it('places every node exactly once', () => {
        const { nodes, edges } = estate();
        const placed = place(nodes, edges);
        expect([...placed.keys()].sort()).toEqual(nodes.map((n) => n.id).sort());
        for (const [x, y] of placed.values()) {
            expect(Number.isFinite(x) && Number.isFinite(y)).toBe(true);
        }
    });

    it('returns an empty map for no nodes', () => {
        expect(spread([], [], new Map()).size).toBe(0);
    });

    it('is deterministic and independent of input order', () => {
        const { nodes, edges } = estate();
        const first = place(nodes, edges);
        const again = place([...nodes].reverse(), [...edges].reverse());
        expect([...again.entries()].sort()).toEqual([...first.entries()].sort());
    });

    it('keeps every pair of nodes a caption apart', () => {
        const { nodes, edges } = estate();
        expect(closest(place(nodes, edges))).toBeGreaterThanOrEqual(0.999);
        const big = tree(200);
        expect(closest(place(big.nodes, big.edges))).toBeGreaterThanOrEqual(0.999);
    });

    it('makes every link long enough to carry its relation between two node rims', () => {
        expect(LINK).toBeGreaterThanOrEqual(44 + 60 + 12);
        const { nodes, edges } = estate();
        expect(Math.min(...linkLengths(nodes, edges, place(nodes, edges)))).toBeGreaterThan(
            LINK - 1,
        );
        const big = tree(60);
        expect(
            Math.min(...linkLengths(big.nodes, big.edges, place(big.nodes, big.edges))),
        ).toBeGreaterThan(LINK - 1);
    });

    it('hangs a node with one link on the outer side of its neighbour, away from the hub', () => {
        const { nodes, edges } = estate();
        const placed = place(nodes, edges);
        const system = at(placed, 'sys:demo/api-worker');
        const hub = at(placed, 'ent:cif-0001');
        const leaf = at(placed, 'def:api-worker-kill');
        const out: Point = [system[0] - hub[0], system[1] - hub[1]];
        const hang: Point = [leaf[0] - system[0], leaf[1] - system[1]];
        expect(out[0] * hang[0] + out[1] * hang[1]).toBeGreaterThan(0);
        expect(distance(leaf, hub)).toBeGreaterThan(distance(system, hub));
    });

    it('fans several one-link nodes of one neighbour without stacking them', () => {
        const { nodes, edges } = estate();
        const placed = place(nodes, edges);
        const queue = at(placed, 'sys:demo/message-queue');
        const hub = at(placed, 'ent:cif-0002');
        const leaves = [
            'cmp:demo/message-queue/broker',
            'wl:demo/statefulset/message-queue',
            'def:queue-dependency-loss',
        ].map((id) => at(placed, id));
        for (const leaf of leaves) {
            expect(distance(leaf, hub)).toBeGreaterThan(distance(queue, hub));
        }
    });

    it('keeps linked nodes at a readable distance', () => {
        const { nodes, edges } = tree(60);
        const placed = place(nodes, edges);
        const lengths = nodes
            .filter((n) => n.parent !== null)
            .map((n) => distance(at(placed, n.id), at(placed, n.parent ?? '')));
        const mean = lengths.reduce((sum, length) => sum + length, 0) / lengths.length;
        expect(mean).toBeLessThan(SPACING * 2.5);
        expect(Math.max(...lengths)).toBeLessThan(SPACING * 6);
    });

    it('treats the spine parent as a link, also without an edge', () => {
        const { nodes, edges } = estate();
        const placed = place(nodes, edges);
        const broker = at(placed, 'cmp:demo/message-queue/broker');
        const system = at(placed, 'sys:demo/message-queue');
        expect(distance(broker, system)).toBeLessThan(SPACING * 3);
    });

    it('gathers nodes without any link on one tidy row below the rest', () => {
        const { nodes, edges } = estate();
        const placed = place(nodes, edges);
        const row = ISOLATED.map((id) => at(placed, id));
        const linked = nodes.filter((n) => !ISOLATED.includes(n.id)).map((n) => at(placed, n.id));
        const lowest = Math.max(...linked.map((p) => p[1]));
        expect(new Set(row.map((p) => p[1])).size).toBe(1);
        expect(row[0][1]).toBeGreaterThanOrEqual(lowest + SPACING);
        const xs = row.map((p) => p[0]).sort((a, b) => a - b);
        expect(xs[1] - xs[0]).toBeCloseTo(SPACING, 1);
        expect(xs[2] - xs[1]).toBeCloseTo(SPACING, 1);
    });

    it('lays out a map of only unlinked nodes as rows', () => {
        const nodes = [node('def:a', 'def'), node('def:b', 'def')];
        const placed = place(nodes, []);
        expect(distance(at(placed, 'def:a'), at(placed, 'def:b'))).toBeCloseTo(SPACING, 1);
    });

    it('uses the width of the canvas: the map is wider than it is tall', () => {
        const { nodes, edges } = tree(60);
        const points = [...place(nodes, edges).values()];
        const span = (axis: 0 | 1) =>
            Math.max(...points.map((p) => p[axis])) - Math.min(...points.map((p) => p[axis]));
        expect(span(0)).toBeGreaterThan(span(1));
    });

    it('leaves a map too large to simulate where the radial layout put it', () => {
        const nodes = [node('org:', 'org')];
        for (let i = 0; i < FORCE_LIMIT; i += 1)
            nodes.push(node(`sys:prod/${i}`, 'system', 'org:'));
        const seed = layoutStar(nodes, []);
        expect(spread(nodes, [], seed)).toBe(seed);
    });

    it('GIVEN a map that falls on one flat line WHEN spread THEN it leans, so the chart has a height', () => {
        const nodes = [node('sys:a', 'system'), node('sys:b', 'system')];
        const edges = [{ from: 'sys:a', to: 'sys:b' }];
        const placed = place(nodes, edges);
        const [a, b] = [placed.get('sys:a') ?? [0, 0], placed.get('sys:b') ?? [0, 0]];
        expect(Math.abs(a[1] - b[1])).toBeGreaterThan(SPACING / 10);
        expect(Math.abs(a[0] - b[0])).toBeGreaterThan(SPACING / 2);
    });
});
