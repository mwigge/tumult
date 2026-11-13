import { describe, expect, it } from 'vitest';
import { MIN_DISTANCE, layoutStar, spineSpans } from './layout.js';
import type { LayoutEdge, LayoutNode } from './layout.js';

const node = (id: string, kind: string, parent: string | null = null): LayoutNode => ({
    id,
    kind,
    parent,
});

/** A small estate: one company, two teams, one entity each, and satellites. */
function small(): { nodes: LayoutNode[]; edges: LayoutEdge[] } {
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
        node('svc:api-worker', 'service', 'sys:demo/api-worker'),
        node('wl:demo/deployment/api-worker', 'workload', 'svc:api-worker'),
        node('wl:demo/statefulset/message-queue', 'workload', 'cmp:demo/message-queue/broker'),
        node('def:queue-dependency-loss', 'def'),
        node('def:api-worker-kill', 'def'),
        node('compliance:DORA/Art.25', 'article'),
        node('gap:network::partition', 'gap'),
    ];
    const edges = [
        { from: 'sys:demo/message-queue', to: 'def:queue-dependency-loss' },
        { from: 'sys:demo/api-worker', to: 'def:api-worker-kill' },
        { from: 'def:queue-dependency-loss', to: 'compliance:DORA/Art.25' },
    ];
    return { nodes, edges };
}

/** 500 nodes with a realistic fan-out, built without randomness. */
function estate(): { nodes: LayoutNode[]; edges: LayoutEdge[] } {
    const nodes: LayoutNode[] = [node('org:', 'org')];
    const edges: LayoutEdge[] = [];
    const pad = (n: number) => String(n).padStart(4, '0');
    let services = 0;
    for (let d = 0; d < 4; d += 1) {
        const domain = `org:domain-${d}`;
        nodes.push(node(domain, 'org', 'org:'));
        for (let t = 0; t < 3; t += 1) {
            const team = `${domain}/team-${t}`;
            nodes.push(node(team, 'org', domain));
            // Uneven teams: the fan-out differs per team on purpose.
            for (let e = 0; e < 2 + ((d + t) % 3); e += 1) {
                const entity = `ent:cif-${pad(nodes.length)}`;
                nodes.push(node(entity, 'entity', team));
                for (let s = 0; s < 2; s += 1) {
                    const system = `sys:prod/system-${pad(nodes.length)}`;
                    nodes.push(node(system, 'system', entity));
                    for (let v = 0; v < 2 + (s % 2); v += 1) {
                        const service = `svc:service-${pad(services)}`;
                        services += 1;
                        nodes.push(node(service, 'service', system));
                        nodes.push(
                            node(`wl:prod/deployment/w-${pad(services)}`, 'workload', service),
                        );
                    }
                }
            }
        }
    }
    const systems = nodes.filter((n) => n.kind === 'system');
    let i = 0;
    while (nodes.length < 496) {
        const def = `def:experiment-${pad(i)}`;
        nodes.push(node(def, 'def'));
        edges.push({ from: systems[i % systems.length].id, to: def });
        i += 1;
    }
    for (const article of ['11', '24', '25', '26']) {
        nodes.push(node(`compliance:DORA/Art.${article}`, 'article'));
    }
    return { nodes, edges };
}

const angleOf = ([x, y]: [number, number]) => Math.atan2(y, x);

/** Whether `angle` lies in `[start, end]`, both measured on the circle. */
function within(angle: number, [start, end]: [number, number]): boolean {
    const turn = 2 * Math.PI;
    const offset = (((angle - start) % turn) + turn) % turn;
    return offset <= end - start + 1e-6 || turn - offset <= 1e-6;
}

describe('layoutStar', () => {
    it('places every node exactly once', () => {
        for (const { nodes, edges } of [small(), estate()]) {
            const positions = layoutStar(nodes, edges);
            expect(positions.size).toBe(nodes.length);
            for (const n of nodes) {
                const at = positions.get(n.id);
                expect(at, n.id).toBeDefined();
                expect(Number.isFinite(at![0]) && Number.isFinite(at![1]), n.id).toBe(true);
            }
        }
    });

    it('puts the organisation at the centre', () => {
        const { nodes, edges } = small();
        expect(layoutStar(nodes, edges).get('org:')).toEqual([0, 0]);
    });

    it('is deterministic and independent of input order', () => {
        const { nodes, edges } = estate();
        const first = layoutStar(nodes, edges);
        const again = layoutStar(nodes, edges);
        const shuffled = layoutStar([...nodes].reverse(), [...edges].reverse());
        expect([...again.entries()]).toEqual([...first.entries()]);
        for (const n of nodes) {
            expect(shuffled.get(n.id), n.id).toEqual(first.get(n.id));
        }
    });

    it('keeps children inside the angular span of their parent', () => {
        for (const { nodes, edges } of [small(), estate()]) {
            const positions = layoutStar(nodes, edges);
            const spans = spineSpans(nodes);
            for (const n of nodes) {
                if (!n.parent || n.parent === 'org:') continue;
                const span = spans.get(n.parent);
                expect(span, n.parent).toBeDefined();
                expect(within(angleOf(positions.get(n.id)!), span!), n.id).toBe(true);
                const own = spans.get(n.id)!;
                expect(own[0]).toBeGreaterThanOrEqual(span![0] - 1e-9);
                expect(own[1]).toBeLessThanOrEqual(span![1] + 1e-9);
            }
        }
    });

    it('moves outward along the spine', () => {
        const { nodes, edges } = small();
        const positions = layoutStar(nodes, edges);
        const radius = (id: string) => Math.hypot(...positions.get(id)!);
        for (const n of nodes) {
            if (!n.parent) continue;
            expect(radius(n.id), n.id).toBeGreaterThan(radius(n.parent));
        }
        // Satellites sit outside every ring of the spine.
        const outer = Math.max(...nodes.filter((n) => n.parent).map((n) => radius(n.id)));
        expect(radius('def:queue-dependency-loss')).toBeGreaterThan(outer);
        expect(radius('compliance:DORA/Art.25')).toBeGreaterThan(
            radius('def:queue-dependency-loss'),
        );
    });

    it('keeps 500 nodes at least the minimum distance apart', () => {
        const { nodes, edges } = estate();
        expect(nodes.length).toBe(500);
        const at = [...layoutStar(nodes, edges).entries()];
        let closest = Infinity;
        for (let a = 0; a < at.length; a += 1) {
            for (let b = a + 1; b < at.length; b += 1) {
                const d = Math.hypot(at[a][1][0] - at[b][1][0], at[a][1][1] - at[b][1][1]);
                closest = Math.min(closest, d);
            }
        }
        expect(closest).toBeGreaterThanOrEqual(MIN_DISTANCE - 0.01);
    });

    it('places nodes whose parent is not in the set', () => {
        const nodes = [
            node('org:', 'org'),
            node('svc:lonely', 'service', 'sys:demo/pruned'),
            node('svc:orphan', 'service'),
        ];
        const positions = layoutStar(nodes, []);
        expect(positions.size).toBe(3);
        expect(positions.get('svc:lonely')).not.toEqual(positions.get('svc:orphan'));
    });

    it('survives a parent cycle without hanging', () => {
        const nodes = [node('svc:a', 'service', 'svc:b'), node('svc:b', 'service', 'svc:a')];
        expect(layoutStar(nodes, []).size).toBe(2);
    });

    it('places two hundred thousand siblings without running out of stack or time', () => {
        const nodes = [node('org:', 'org')];
        for (let i = 0; i < 200_000; i += 1) nodes.push(node(`org:u${i}`, 'org', 'org:'));
        const started = performance.now();
        const positions = layoutStar(nodes, []);
        expect(positions.size).toBe(nodes.length);
        expect(performance.now() - started).toBeLessThan(15_000);
    }, 30_000);

    it('places a hundred and fifty thousand satellites', () => {
        const nodes = [node('org:', 'org'), node('sys:demo/a', 'system', 'org:')];
        for (let i = 0; i < 150_000; i += 1) nodes.push(node(`def:name/d${i}`, 'def'));
        expect(layoutStar(nodes, []).size).toBe(nodes.length);
    }, 30_000);

    it('returns an empty map for no nodes', () => {
        expect(layoutStar([], []).size).toBe(0);
    });
});
