// The Star Map as lists: the same nodes the canvas draws, in an order a
// keyboard or a screen reader can walk.

import type { StarNode } from '../types.js';
import { KINDS } from './style.js';

const RANK = new Map<string, number>(KINDS.map((kind, i) => [kind, i]));
const rank = (n: StarNode) => RANK.get(n.kind) ?? KINDS.length;

function before(a: StarNode, b: StarNode): number {
    if (rank(a) !== rank(b)) return rank(a) - rank(b);
    if (a.label !== b.label) return a.label < b.label ? -1 : 1;
    return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
}

/**
 * Nodes from the centre outwards, then by label. `filter` keeps the nodes
 * whose label, id or kind contains it, whatever the case.
 */
export function listNodes(nodes: StarNode[], filter: string): StarNode[] {
    const wanted = filter.trim().toLowerCase();
    const has = (text: string) => text.toLowerCase().includes(wanted);
    return nodes.filter((n) => has(n.label) || has(n.id) || has(n.kind)).sort(before);
}
