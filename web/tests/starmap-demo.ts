// A Star Map the size and shape of the seeded Krönika demo: 51 nodes and
// 38 edges, with only the node kinds and relations the daemon reports
// there. Org units under their parent unit and components in their system
// are joined by `parent` alone, as in the real payload.

const DAY_NS = 86_400_000_000_000;

const noRisks = {
    open: 0,
    suppressed: 0,
    worst: null as string | null,
    suppression_expires_ns: null as number | null,
    suppressed_by: null as string | null,
    suppression_reason: null as string | null,
    recommended: [] as string[],
};

const noEvidence = {
    runs: 0,
    failed: 0,
    manual: 0,
    last_run_ns: null as number | null,
    by_discipline: {} as Record<string, string>,
};

function node(id: string, kind: string, label: string, parent: string | null, over = {}) {
    return {
        id,
        kind,
        label,
        parent,
        env: kind === 'org' || kind === 'entity' || kind === 'def' ? null : 'demo',
        tier: null,
        org_kind: kind === 'org' ? 'team' : null,
        criticality_weight: 1,
        conformance: null,
        last_seen_ns: null,
        posture: null,
        evidence: noEvidence,
        gap: null as string | null,
        risks: noRisks,
        slo: null,
        slo_count: 0,
        dora: null,
        maturity: null,
        conflict: false,
        flags: [],
        rollup: null,
        declared: false,
        children: 0,
        members: null,
        ...over,
    };
}

const edge = (from: string, rel: string, to: string, source = 'declared') => ({
    id: `${from}|${rel}|${to}`,
    from,
    to,
    rel,
    source,
});

/** [system, owner (entity or org unit), state] */
const SYSTEMS: [string, string, 'passed' | 'failed' | 'missing' | 'stale' | 'suppressed'][] = [
    ['message-queue', 'ent:cif-0004', 'failed'],
    ['config-service', 'ent:cif-0002', 'failed'],
    ['cdn', 'ent:cif-0006', 'stale'],
    ['session-cache', 'ent:cif-0005', 'missing'],
    ['api-worker', 'ent:cif-0001', 'suppressed'],
    ['batch-worker', 'ent:cif-0001', 'passed'],
    ['service', 'ent:cif-0003', 'passed'],
    ['checkout', 'ent:cif-0003', 'passed'],
    ['worker', 'org:platform/compute/runtime-team', 'passed'],
    ['storage', 'org:platform/data/data-team', 'passed'],
    ['host', 'org:platform/compute', 'passed'],
];

const ENTITIES: [string, string][] = [
    ['cif-0001', 'org:platform/compute/runtime-team'],
    ['cif-0002', 'org:platform/data/data-team'],
    ['cif-0003', 'org:platform/compute/runtime-team'],
    ['cif-0004', 'org:platform/data/data-team'],
    ['cif-0006', 'org:platform/edge/edge-team'],
    ['cif-0005', 'org:(unassigned)'],
];

const COMPONENTS: [string, string][] = [
    ['message-queue', 'broker'],
    ['config-service', 'store'],
    ['cdn', 'edge-cache'],
    ['session-cache', 'redis'],
    ['api-worker', 'http'],
    ['batch-worker', 'scheduler'],
];

const WORKLOADS: [string, string, string][] = [
    ['message-queue', 'statefulset', 'message-queue'],
    ['config-service', 'deployment', 'config-service'],
    ['cdn', 'deployment', 'cdn'],
    ['session-cache', 'statefulset', 'session-cache'],
    ['api-worker', 'deployment', 'api-worker'],
    ['batch-worker', 'cronjob', 'batch-worker'],
    ['checkout', 'deployment', 'checkout'],
];

/** [definition, systems it tested, whether its last run broke the first] */
const DEFS: [string, string[], boolean][] = [
    ['message-queue dependency loss with restart', ['message-queue', 'api-worker'], true],
    ['config-service pod kill', ['config-service'], true],
    ['cdn origin latency', ['cdn'], false],
    ['api-worker cpu stress', ['api-worker'], false],
    ['batch-worker node drain', ['batch-worker'], false],
    ['service network delay', ['service'], false],
    ['checkout load ramp', ['checkout'], false],
    ['worker memory pressure', ['worker'], false],
    ['storage disk fill', ['storage'], false],
    ['host reboot', ['host'], false],
    ['checkout zone failover', ['checkout'], false],
    ['never-ran dns outage', [], false],
];

const sys = (name: string) => `sys:demo/${name}`;
const def = (name: string) => `def:name/${name}`;

function systemState(state: (typeof SYSTEMS)[number][2], nowNs: number) {
    const ran = { ...noEvidence, runs: 3, last_run_ns: nowNs - DAY_NS };
    if (state === 'missing') return { gap: 'missing', declared: true };
    if (state === 'stale') return { gap: 'stale', evidence: noEvidence };
    if (state === 'failed') {
        return {
            evidence: { ...ran, failed: 1 },
            risks: { ...noRisks, open: 3, worst: 'high' },
        };
    }
    if (state === 'suppressed') {
        return {
            evidence: ran,
            risks: {
                ...noRisks,
                suppressed: 1,
                suppression_expires_ns: nowNs + 60 * DAY_NS,
                suppressed_by: 'demo',
                suppression_reason: 'accepted for the demo',
            },
        };
    }
    return { evidence: ran };
}

function demoNodes(nowNs: number) {
    const nodes = [
        node('org:', 'org', '(company)', null, { org_kind: 'company' }),
        node('org:platform', 'org', 'platform', 'org:', { org_kind: 'domain' }),
        node('org:platform/data', 'org', 'data', 'org:platform', { org_kind: 'unit' }),
        node('org:platform/edge', 'org', 'edge', 'org:platform', { org_kind: 'unit' }),
        node('org:platform/compute', 'org', 'compute', 'org:platform', { org_kind: 'unit' }),
        node('org:platform/data/data-team', 'org', 'data-team', 'org:platform/data'),
        node('org:platform/edge/edge-team', 'org', 'edge-team', 'org:platform/edge'),
        node('org:platform/compute/runtime-team', 'org', 'runtime-team', 'org:platform/compute'),
        node('org:(unassigned)', 'org', '(unassigned)', 'org:', { org_kind: 'unassigned' }),
    ];
    const failing = new Set(['cif-0004', 'cif-0002']);
    for (const [name, owner] of ENTITIES) {
        const over = failing.has(name)
            ? { evidence: { ...noEvidence, runs: 3, failed: 1 } }
            : name === 'cif-0005'
              ? { gap: 'missing' }
              : { evidence: { ...noEvidence, runs: 3 } };
        nodes.push(node(`ent:${name}`, 'entity', name, owner, over));
    }
    for (const [name, owner, state] of SYSTEMS) {
        nodes.push(node(sys(name), 'system', name, owner, systemState(state, nowNs)));
    }
    for (const [system, name] of COMPONENTS) {
        nodes.push(node(`cmp:demo/${system}/${name}`, 'component', name, sys(system)));
    }
    for (const [system, kind, name] of WORKLOADS) {
        nodes.push(node(`wl:demo/${kind}/${name}`, 'workload', name, sys(system)));
    }
    for (const [name, , broke] of DEFS) {
        const evidence = { ...noEvidence, runs: 1, failed: broke ? 1 : 0 };
        nodes.push(
            node(def(name), 'def', name, null, name.startsWith('never') ? {} : { evidence }),
        );
    }
    return nodes;
}

function demoEdges() {
    const edges = [];
    for (const [name, owner] of ENTITIES) {
        if (owner !== 'org:(unassigned)') edges.push(edge(owner, 'owns', `ent:${name}`));
    }
    for (const [name, owner] of SYSTEMS) {
        const owned = owner.startsWith('org:');
        edges.push(owned ? edge(owner, 'owns', sys(name)) : edge(owner, 'implements', sys(name)));
    }
    for (const [system, kind, name] of WORKLOADS) {
        edges.push(edge(sys(system), 'runs_on', `wl:demo/${kind}/${name}`));
    }
    for (const [name, systems, broke] of DEFS) {
        for (const system of systems) {
            edges.push(edge(sys(system), 'tested_by', def(name), 'evidence'));
        }
        if (broke) edges.push(edge(def(name), 'broke', sys(systems[0]), 'evidence'));
    }
    edges.push(edge(sys('checkout'), 'depends_on', sys('session-cache')));
    return edges;
}

/** The nodes and edges of the demo-sized map, as `GET /api/starmap` gives them. */
export function demoStar(nowNs: number) {
    return { nodes: demoNodes(nowNs), edges: demoEdges() };
}
