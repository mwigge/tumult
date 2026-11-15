// Force pass over the radial layout: a pure, repeatable simulation that
// pulls linked nodes to a readable distance and pushes every pair apart
// far enough for a round node and its caption. It starts from the radial
// positions, runs a fixed number of steps and uses no randomness, so the
// map still looks the same on every load. Layout units are pixels at
// zoom 1.

import type { LayoutEdge, LayoutNode, Point } from './layout.js';

/** No two nodes end up closer than this. */
export const SPACING = 90;
/** Above this many linked nodes the radial layout is kept as it is. */
export const FORCE_LIMIT = 600;

/** Length a link settles at when nothing else pulls on it. */
const LINK = SPACING * 1.3;
const STEPS = 300;
/** Pull toward the centre; stronger up and down, so the map grows wide. */
const GRAVITY: Point = [0.02, 0.04];
const SETTLE_PASSES = 400;
/** Slack over `SPACING` so rounding never breaks the guarantee. */
const PAD = 0.5;
const ROW_GAP = SPACING * 1.5;
const ROW_MIN = 5;

interface Body {
    id: string;
    x: number;
    y: number;
    dx: number;
    dy: number;
}

type Pair = [number, number];

const round = (v: number) => Math.round(v * 100) / 100;

/** Every linked pair once: edges and spine parents, both ends on the map. */
function pairsOf(nodes: LayoutNode[], edges: LayoutEdge[]): [string, string][] {
    const ids = new Set(nodes.map((n) => n.id));
    const seen = new Map<string, [string, string]>();
    const add = (a: string, b: string) => {
        if (a === b || !ids.has(a) || !ids.has(b)) return;
        const pair: [string, string] = a < b ? [a, b] : [b, a];
        seen.set(pair.join('\u0000'), pair);
    };
    for (const n of nodes) {
        if (n.parent) add(n.id, n.parent);
    }
    for (const e of edges) add(e.from, e.to);
    return [...seen.keys()].sort().map((key) => seen.get(key) as [string, string]);
}

/** Bodies at the radial positions, scaled to the room the nodes need. */
function seedBodies(ids: string[], seed: Map<string, Point>): Body[] {
    const at = ids.map((id) => seed.get(id) ?? [0, 0]);
    const reach = at.reduce((most, p) => Math.max(most, Math.hypot(p[0], p[1])), 0);
    const room = SPACING * Math.sqrt(ids.length) * 0.7;
    const scale = reach > 0 ? room / reach : 1;
    return ids.map((id, i) => ({ id, x: at[i][0] * scale, y: at[i][1] * scale, dx: 0, dy: 0 }));
}

/** The vector from `b` to `a`; two bodies on one spot get a fixed one. */
function apart(a: Body, b: Body, i: number, j: number): [number, number, number] {
    const x = a.x - b.x;
    const y = a.y - b.y;
    const d = Math.hypot(x, y);
    if (d > 1e-6) return [x, y, d];
    const turn = (i * 31 + j * 17) % 360;
    return [Math.cos(turn) * 0.01, Math.sin(turn) * 0.01, 0.01];
}

function repel(bodies: Body[]): void {
    for (let i = 0; i < bodies.length; i += 1) {
        for (let j = i + 1; j < bodies.length; j += 1) {
            const [x, y, d] = apart(bodies[i], bodies[j], i, j);
            const push = (LINK * LINK) / (d * d);
            bodies[i].dx += x * push;
            bodies[i].dy += y * push;
            bodies[j].dx -= x * push;
            bodies[j].dy -= y * push;
        }
    }
}

function attract(bodies: Body[], pairs: Pair[]): void {
    for (const [i, j] of pairs) {
        const [x, y, d] = apart(bodies[i], bodies[j], i, j);
        const pull = d / LINK;
        bodies[i].dx -= x * pull;
        bodies[i].dy -= y * pull;
        bodies[j].dx += x * pull;
        bodies[j].dy += y * pull;
    }
}

/** Move every body by its force, at most `heat` far, and clear the force. */
function move(bodies: Body[], heat: number): void {
    for (const b of bodies) {
        b.dx -= b.x * GRAVITY[0] * LINK;
        b.dy -= b.y * GRAVITY[1] * LINK;
        const force = Math.hypot(b.dx, b.dy);
        const step = force > 0 ? Math.min(force, heat) / force : 0;
        b.x += b.dx * step;
        b.y += b.dy * step;
        b.dx = 0;
        b.dy = 0;
    }
}

/** Push apart the pairs that are still too close; true when none was. */
function separate(bodies: Body[]): boolean {
    const need = SPACING + PAD;
    let clean = true;
    for (let i = 0; i < bodies.length; i += 1) {
        for (let j = i + 1; j < bodies.length; j += 1) {
            const [x, y, d] = apart(bodies[i], bodies[j], i, j);
            if (d >= need) continue;
            clean = false;
            const shift = (need - d) / 2 / d;
            bodies[i].x += x * shift;
            bodies[i].y += y * shift;
            bodies[j].x -= x * shift;
            bodies[j].y -= y * shift;
        }
    }
    return clean;
}

function simulate(bodies: Body[], pairs: Pair[]): void {
    for (let step = 0; step < STEPS; step += 1) {
        repel(bodies);
        attract(bodies, pairs);
        move(bodies, LINK * (1 - step / STEPS));
    }
    for (let pass = 0; pass < SETTLE_PASSES; pass += 1) {
        if (separate(bodies)) return;
    }
}

interface Box {
    left: number;
    right: number;
    bottom: number;
}

function boxOf(bodies: Body[]): Box | null {
    if (bodies.length === 0) return null;
    const box = { left: Infinity, right: -Infinity, bottom: -Infinity };
    for (const b of bodies) {
        box.left = Math.min(box.left, b.x);
        box.right = Math.max(box.right, b.x);
        box.bottom = Math.max(box.bottom, b.y);
    }
    return box;
}

/** Nodes without any link, on centred rows below the linked part. */
function placeRows(ids: string[], box: Box | null, placed: Map<string, Point>): void {
    if (ids.length === 0) return;
    const width = box ? box.right - box.left : 0;
    const perRow = Math.max(ROW_MIN, Math.floor(width / SPACING) + 1);
    const centre = box ? (box.left + box.right) / 2 : 0;
    const top = box ? box.bottom + ROW_GAP : 0;
    ids.forEach((id, i) => {
        const row = Math.floor(i / perRow);
        const inRow = Math.min(perRow, ids.length - row * perRow);
        const x = centre + ((i % perRow) - (inRow - 1) / 2) * SPACING;
        placed.set(id, [round(x), round(top + row * SPACING)]);
    });
}

/** How steeply a map that fell on one flat line leans. */
const LEAN = 0.25;

/**
 * Lean a map whose nodes all sit at one height: a chart of no height
 * draws its edges beside the nodes they join.
 */
function lean(bodies: Body[]): void {
    if (bodies.length < 2) return;
    const ys = bodies.map((b) => b.y);
    if (Math.max(...ys) - Math.min(...ys) >= 1) return;
    const mid = bodies.reduce((sum, b) => sum + b.x, 0) / bodies.length;
    for (const b of bodies) b.y += (b.x - mid) * LEAN;
}

/**
 * Positions for every node, from the radial `seed`. A node hangs on its
 * spine parent and on the other end of each of its edges; a node with
 * neither goes on a row below the map, ordered by kind and id. The
 * result does not depend on the order of either list.
 */
export function spread(
    nodes: LayoutNode[],
    edges: LayoutEdge[],
    seed: Map<string, Point>,
): Map<string, Point> {
    const pairs = pairsOf(nodes, edges);
    const linked = new Set(pairs.flat());
    if (linked.size > FORCE_LIMIT) return seed;
    const sorted = [...nodes].sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0));
    const ids = sorted.filter((n) => linked.has(n.id)).map((n) => n.id);
    const index = new Map(ids.map((id, i) => [id, i]));
    const bodies = seedBodies(ids, seed);
    simulate(
        bodies,
        pairs.map(([a, b]): Pair => [index.get(a) ?? 0, index.get(b) ?? 0]),
    );
    lean(bodies);
    const placed = new Map<string, Point>();
    for (const b of bodies) placed.set(b.id, [round(b.x), round(b.y)]);
    const alone = sorted
        .filter((n) => !linked.has(n.id))
        .sort((a, b) => (a.kind === b.kind ? 0 : a.kind < b.kind ? -1 : 1))
        .map((n) => n.id);
    placeRows(alone, boxOf(bodies), placed);
    return placed;
}
