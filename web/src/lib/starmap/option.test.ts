import { describe, expect, it } from 'vitest';
import { chartTokens } from '../echarts.js';
import type { StarEdge, StarKind, StarNode, StarRel } from '../types.js';
import type { Point } from './layout.js';
import {
    CAPTION_LIMIT,
    captionsShown,
    legendKinds,
    spinePairs,
    starSeries,
    statusKey,
} from './option.js';
import type { StarView } from './option.js';
import { KIND_COLOR } from './style.js';

function node(id: string, kind: StarKind, parent: string | null = null): StarNode {
    return {
        id,
        kind,
        label: id.slice(id.lastIndexOf('/') + 1),
        parent,
        env: 'demo',
        tier: null,
        org_kind: null,
        criticality_weight: 1,
        conformance: null,
        last_seen_ns: null,
        posture: null,
        evidence: { runs: 0, failed: 0, manual: 0, last_run_ns: null, by_discipline: {} },
        gap: null,
        risks: {
            open: 0,
            suppressed: 0,
            worst: null,
            suppression_expires_ns: null,
            suppressed_by: null,
            suppression_reason: null,
            recommended: [],
        },
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
    };
}

const edge = (from: string, rel: StarRel, to: string): StarEdge => ({
    id: `${from}|${rel}|${to}`,
    from,
    to,
    rel,
    source: 'declared',
});

const ORG = 'org:';
const TEAM = 'org:team';
const ENT = 'ent:cif-0001';
const SYS = 'sys:demo/queue';
const CMP = 'cmp:demo/queue/broker';
const DEF = 'def:name/queue-loss';
const LONE = 'def:name/never-ran';

const nodes = [
    node(ORG, 'org'),
    node(TEAM, 'org', ORG),
    node(ENT, 'entity', TEAM),
    node(SYS, 'system', ENT),
    node(CMP, 'component', SYS),
    node(DEF, 'def'),
    node(LONE, 'def'),
];

const edges = [
    edge(TEAM, 'owns', ENT),
    edge(ENT, 'implements', SYS),
    edge(SYS, 'tested_by', DEF),
    edge(DEF, 'broke', SYS),
];

const positions = new Map<string, Point>(nodes.map((n, i) => [n.id, [i * 100, i * 10]]));

function view(over: Partial<StarView> = {}, context: Partial<StarView['context']> = {}): StarView {
    return {
        nodes,
        edges,
        positions,
        hidden: new Set(),
        focused: false,
        light: false,
        ...over,
        context: {
            lens: 'evidence',
            tokens: chartTokens('tumult-dark'),
            selected: null,
            zoom: 1,
            nowNs: 0,
            ...context,
        },
    };
}

const linkOf = (series: ReturnType<typeof starSeries>, id: string) => {
    const link = series.links.find((l) => l.id === id);
    if (!link) throw new Error(`no link ${id}`);
    return link;
};

const itemOf = (series: ReturnType<typeof starSeries>, id: string) => {
    const item = series.data.find((d) => d.id === id);
    if (!item) throw new Error(`no item ${id}`);
    return item;
};

describe('spinePairs', () => {
    it('lists the parents no edge names, and only those', () => {
        expect(spinePairs(nodes, edges)).toEqual([
            { child: CMP, parent: SYS },
            { child: TEAM, parent: ORG },
        ]);
    });

    it('counts an edge in either direction and skips a parent that is not on the map', () => {
        const some = [node(SYS, 'system', 'ent:gone'), node(CMP, 'component', SYS)];
        expect(spinePairs(some, [edge(CMP, 'implements', SYS)])).toEqual([]);
    });
});

describe('captionsShown', () => {
    it('writes the relations at normal zoom on a map that is not dense', () => {
        expect(captionsShown(CAPTION_LIMIT, 1, false)).toBe(true);
        expect(captionsShown(CAPTION_LIMIT + 1, 1, false)).toBe(false);
        expect(captionsShown(10, 0, false)).toBe(false);
    });

    it('always writes them in focus mode', () => {
        expect(captionsShown(CAPTION_LIMIT + 1, 0, true)).toBe(true);
    });
});

describe('legendKinds', () => {
    it('lists the kinds on the map with their colour and count, centre first', () => {
        expect(legendKinds(nodes)).toEqual([
            { kind: 'org', name: 'org unit', color: KIND_COLOR.org, count: 2 },
            { kind: 'entity', name: 'ICT entity', color: KIND_COLOR.entity, count: 1 },
            { kind: 'system', name: 'system', color: KIND_COLOR.system, count: 1 },
            { kind: 'component', name: 'component', color: KIND_COLOR.component, count: 1 },
            { kind: 'def', name: 'experiment', color: KIND_COLOR.def, count: 2 },
        ]);
    });
});

describe('statusKey', () => {
    it('explains the rings of the lens in force, each with its mark', () => {
        expect(statusKey('evidence')).toEqual([
            { status: 'ok', mark: '\u2713', text: 'runs passed' },
            { status: 'fail', mark: '\u2715', text: 'a run failed' },
        ]);
        expect(statusKey('risk').map((k) => k.status)).toEqual(['warn', 'fail']);
        expect(statusKey('conformance').map((k) => k.status)).toEqual(['ok', 'warn', 'fail']);
    });
});

describe('starSeries', () => {
    it('is a roamable graph at fixed positions that highlights a neighbourhood on hover', () => {
        const series = starSeries(view());
        expect(series.id).toBe('star');
        expect(series.type).toBe('graph');
        expect(series.layout).toBe('none');
        expect(series.roam).toBe(true);
        expect(series.emphasis.focus).toBe('adjacency');
        expect(series.emphasis.edgeLabel.show).toBe(true);
        expect(series.emphasis.label.show).toBe(true);
    });

    it('draws one item per node at its position', () => {
        const series = starSeries(view());
        expect(series.data.map((d) => d.id)).toEqual(nodes.map((n) => n.id));
        expect([itemOf(series, SYS).x, itemOf(series, SYS).y]).toEqual([300, 30]);
    });

    it('captions a node below it with its short name', () => {
        const series = starSeries(view());
        expect(series.label.position).toBe('bottom');
        expect(series.label.formatter({ data: { raw: nodes[3] } })).toBe('queue');
        expect(series.label.formatter({})).toBe('');
    });

    it('draws every API edge and a plain line for each parent without one', () => {
        const series = starSeries(view());
        expect(series.links.map((l) => l.id)).toEqual([
            ...edges.map((e) => e.id),
            `spine|${CMP}`,
            `spine|${TEAM}`,
        ]);
        expect(series.edgeLabel.fontFamily).toBe('monospace');
    });

    it('keeps a single edge straight and bends parallel ones apart', () => {
        const twice = [...edges, edge(TEAM, 'depends_on', ENT)];
        const series = starSeries(view({ edges: twice }));
        expect(linkOf(series, `${ENT}|implements|${SYS}`).lineStyle.curveness).toBe(0);
        // Same direction: one to each side.
        expect(linkOf(series, `${TEAM}|depends_on|${ENT}`).lineStyle.curveness).toBe(0.2);
        expect(linkOf(series, `${TEAM}|owns|${ENT}`).lineStyle.curveness).toBe(-0.2);
        // Opposite directions: the same bend puts them on opposite sides.
        expect(linkOf(series, `${SYS}|tested_by|${DEF}`).lineStyle.curveness).toBe(0.2);
        expect(linkOf(series, `${DEF}|broke|${SYS}`).lineStyle.curveness).toBe(0.2);
    });

    it('hides the relation captions when zoomed out, but not in focus mode', () => {
        const far = starSeries(view({}, { zoom: 0 }));
        expect(linkOf(far, `${TEAM}|owns|${ENT}`).label.show).toBe(false);
        const focused = starSeries(view({ focused: true }, { zoom: 0 }));
        expect(linkOf(focused, `${TEAM}|owns|${ENT}`).label.show).toBe(true);
        expect(linkOf(focused, `spine|${CMP}`).label.show).toBe(false);
    });

    it('leaves out a hidden kind and every link that touches it', () => {
        const series = starSeries(view({ hidden: new Set<StarKind>(['def', 'component']) }));
        expect(series.data.map((d) => d.id)).toEqual([ORG, TEAM, ENT, SYS]);
        expect(series.links.map((l) => l.id)).toEqual([
            `${TEAM}|owns|${ENT}`,
            `${ENT}|implements|${SYS}`,
            `spine|${TEAM}`,
        ]);
    });

    it('dims everything outside the neighbourhood of the selected node', () => {
        const series = starSeries(view({}, { selected: SYS, zoom: 0 }));
        for (const id of [SYS, ENT, DEF, CMP]) {
            expect(itemOf(series, id).itemStyle.opacity, id).toBeUndefined();
        }
        for (const id of [ORG, TEAM, LONE]) {
            expect(itemOf(series, id).itemStyle.opacity, id).toBe(0.2);
            expect(itemOf(series, id).label.show, id).toBe(false);
        }
        const away = linkOf(series, `${TEAM}|owns|${ENT}`);
        expect(away.lineStyle.opacity).toBe(0.1);
        expect(away.label.show).toBe(false);
        // The edges of the selected node keep their caption, zoomed out too.
        const own = linkOf(series, `${SYS}|tested_by|${DEF}`);
        expect(own.lineStyle.opacity).toBeGreaterThan(0.7);
        expect(own.label.show).toBe(true);
        expect(itemOf(series, ENT).label.show).toBe(true);
    });

    it('dims nothing when the selected node is not on the map', () => {
        const series = starSeries(view({}, { selected: 'sys:demo/gone' }));
        for (const item of series.data) expect(item.itemStyle.opacity, item.id).toBeUndefined();
    });

    it('dims nothing in focus mode, where the map is already one neighbourhood', () => {
        const series = starSeries(view({ focused: true }, { selected: SYS }));
        for (const item of series.data) expect(item.itemStyle.opacity, item.id).toBeUndefined();
        for (const link of series.links)
            expect(link.lineStyle.opacity, link.id).toBeGreaterThan(0.4);
    });

    it('blurs less on the light theme, where faint lines vanish', () => {
        expect(starSeries(view()).blur.lineStyle.opacity).toBe(0.05);
        expect(starSeries(view({ light: true })).blur.lineStyle.opacity).toBe(0.12);
    });

    it('takes the caption colours from the theme in force', () => {
        const light = chartTokens('tumult-light');
        const series = starSeries(view({ light: true }, { tokens: light }));
        expect(series.label.color).toBe(light.text);
        expect(series.edgeLabel.color).toBe(light.text);
    });

    it('outlines every caption in the panel colour, so a line under it does not hide it', () => {
        const light = chartTokens('tumult-light');
        const series = starSeries(view({ light: true }, { tokens: light }));
        for (const label of [series.label, series.edgeLabel]) {
            expect(label.textBorderColor).toBe(light.tooltip.backgroundColor);
            expect(label.textBorderWidth).toBeGreaterThanOrEqual(2);
        }
    });
});
