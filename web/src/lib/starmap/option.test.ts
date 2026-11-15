import { describe, expect, it } from 'vitest';
import { chartTokens } from '../echarts.js';
import type { StarEdge, StarKind, StarNode, StarRel } from '../types.js';
import type { Point } from './layout.js';
import {
    CAPTION_LIMIT,
    STRETCH_LIMIT,
    VIEW_PAD,
    captionBox,
    captionFits,
    captionsShown,
    fillCanvas,
    fitScale,
    overlaps,
    legendKinds,
    nameLimit,
    spinePairs,
    starSeries,
    statusKey,
    twins,
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

/** A canvas of the shape of `positions`: one pixel per layout unit. */
const canvas = { width: 600 + 2 * VIEW_PAD[0], height: 60 + 2 * VIEW_PAD[1] };

function view(over: Partial<StarView> = {}, context: Partial<StarView['context']> = {}): StarView {
    return {
        nodes,
        edges,
        positions,
        canvas,
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

describe('nameLimit', () => {
    it('is the characters that fit between two nodes side by side at the drawn scale', () => {
        expect(nameLimit(1)).toBe(16);
        expect(nameLimit(2)).toBe(16);
        expect(nameLimit(0.8)).toBe(15);
        expect(nameLimit(0.55)).toBe(9);
        expect(nameLimit(0.1)).toBe(6);
    });
});

describe('fitScale', () => {
    it('is the pixels per layout unit when the map is fitted inside the padded canvas', () => {
        expect(fitScale(positions, canvas)).toBeCloseTo(1, 5);
        expect(fitScale(positions, { width: 300 + 2 * VIEW_PAD[0], height: 2000 })).toBeCloseTo(
            0.5,
            5,
        );
        // The height binds when the canvas is low.
        expect(fitScale(positions, { width: 5000, height: 30 + 2 * VIEW_PAD[1] })).toBeCloseTo(
            0.5,
            5,
        );
    });

    it('is 1 for a map without extent or a canvas not measured yet', () => {
        expect(fitScale(new Map([['a', [5, 5]]]), canvas)).toBe(1);
        expect(fitScale(new Map(), canvas)).toBe(1);
        expect(fitScale(positions, { width: 0, height: 0 })).toBe(1);
    });
});

describe('captionFits', () => {
    it('is true when the caption has room between the two node rims', () => {
        // 200 long, minus two radii of 20, leaves 160 for ten characters.
        expect(captionFits('IMPLEMENTS', [0, 0], [200, 0], 40, 40, 1)).toBe(true);
    });

    it('is false when the drawn edge is too short for it', () => {
        expect(captionFits('IMPLEMENTS', [0, 0], [100, 0], 40, 40, 1)).toBe(false);
        expect(captionFits('OWNS', [0, 0], [100, 0], 40, 40, 1)).toBe(true);
        // The same edge drawn at half the size.
        expect(captionFits('IMPLEMENTS', [0, 0], [200, 0], 40, 40, 0.5)).toBe(false);
    });
});

describe('fillCanvas', () => {
    const flat = new Map<string, Point>([
        ['a', [0, 0]],
        ['b', [400, 100]],
    ]);
    const room = (width: number, height: number) => ({
        width: width + 2 * VIEW_PAD[0],
        height: height + 2 * VIEW_PAD[1],
    });

    it('stretches the map up or down to the shape of the canvas, never closer together', () => {
        expect(fillCanvas(flat, room(400, 130)).get('b')).toEqual([400, 130]);
        expect(fillCanvas(flat, room(400, 130)).get('a')).toEqual([0, 0]);
        // A high map in a wide canvas grows sideways.
        expect(fillCanvas(flat, room(1000, 200)).get('b')).toEqual([500, 100]);
    });

    it('stretches by no more than STRETCH_LIMIT, so the map keeps its look', () => {
        expect(fillCanvas(flat, room(400, 4000)).get('b')).toEqual([400, 100 * STRETCH_LIMIT]);
    });

    it('leaves a map that has the shape of the canvas, or no extent, as it is', () => {
        expect(fillCanvas(flat, room(800, 200))).toEqual(flat);
        expect(fillCanvas(flat, { width: 0, height: 0 })).toEqual(flat);
        const line = new Map<string, Point>([['a', [5, 5]]]);
        expect(fillCanvas(line, room(400, 400))).toEqual(line);
    });
});

describe('captionBox', () => {
    it('is the box of a relation written just above the middle of its edge', () => {
        const flat = captionBox('OWNS', [0, 0], [200, 0], 0);
        expect(flat.x).toBe(100);
        expect(flat.y).toBeLessThan(-5);
        expect(flat.y).toBeGreaterThan(-15);
        expect(flat.w).toBeGreaterThan(flat.h);
        // Written the same way whichever end the edge starts from.
        expect(captionBox('OWNS', [200, 0], [0, 0], 0)).toEqual(flat);
        const up = captionBox('OWNS', [0, 0], [0, 200], 0);
        expect(up.y).toBe(100);
        expect([up.w, up.h]).toEqual([flat.h, flat.w]);
    });

    it('sits in the middle of the drawn line, which ends at the two node rims', () => {
        const even = captionBox('OWNS', [0, 0], [200, 0], 0);
        const uneven = captionBox('OWNS', [0, 0], [200, 0], 0, [20, 60]);
        // The line runs from 10 to 170: its middle is at 90.
        expect([uneven.x, uneven.y]).toEqual([90, even.y]);
    });

    it('follows a bent edge to the middle of its curve', () => {
        const flat = captionBox('OWNS', [0, 0], [200, 0], 0);
        const bent = captionBox('OWNS', [0, 0], [200, 0], 0.2);
        expect([bent.x, bent.y]).toEqual([100, flat.y - 20]);
    });
});

describe('overlaps', () => {
    const box = { x: 0, y: 0, w: 10, h: 5 };

    it('is true when two boxes share any area', () => {
        expect(overlaps(box, [{ x: 15, y: 8, w: 10, h: 5 }])).toBe(true);
        expect(overlaps(box, [{ x: 100, y: 0, w: 10, h: 5 }, box])).toBe(true);
    });

    it('is false when they are apart, or only touch', () => {
        expect(overlaps(box, [])).toBe(false);
        expect(overlaps(box, [{ x: 20, y: 0, w: 10, h: 5 }])).toBe(false);
        expect(overlaps(box, [{ x: 0, y: 11, w: 10, h: 5 }])).toBe(false);
    });
});

describe('twins', () => {
    const wl = 'wl:demo/deployment/queue';
    const far = 'cmp:demo/other/queue';
    const some = [...nodes, node(wl, 'workload', SYS), node(far, 'component', ORG)];

    it('finds the nodes that share their name with a node one link away', () => {
        const pairs = [
            { source: SYS, target: wl },
            { source: far, target: ORG },
        ];
        expect([...twins(some, pairs)].sort()).toEqual([SYS, wl].sort());
    });

    it('compares names without regard to case and ignores nodes that are not linked', () => {
        const shout = { ...node('wl:demo/deployment/QUEUE', 'workload'), label: 'QUEUE' };
        const pairs = [{ source: shout.id, target: SYS }];
        expect([...twins([...nodes, shout], pairs)].sort()).toEqual([SYS, shout.id].sort());
        expect(twins(some, []).size).toBe(0);
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

    it('cuts a caption shorter when the map is drawn small', () => {
        const long = { ...nodes[3], label: 'queue-of-messages' };
        const text = (width: number) =>
            starSeries(view({ canvas: { ...canvas, width } })).label.formatter({
                data: { raw: long },
            });
        expect(text(canvas.width)).toBe('queue-of-messag…');
        expect(text(330 + 2 * VIEW_PAD[0])).toBe('queue-of…');
    });

    it('draws every API edge and a PART_OF arrow to each parent without one', () => {
        const series = starSeries(view());
        expect(series.links.map((l) => l.id)).toEqual([
            ...edges.map((e) => e.id),
            `spine|${CMP}`,
            `spine|${TEAM}`,
        ]);
        expect(series.edgeLabel.fontFamily).toBe('monospace');
        const part = linkOf(series, `spine|${CMP}`);
        expect([part.source, part.target]).toEqual([CMP, SYS]);
        expect(part.symbol).toEqual(['none', 'arrow']);
        expect(part.label).toEqual({ show: true, formatter: 'PART_OF' });
    });

    it('draws the nodes where the map fills the canvas', () => {
        const at = new Map<string, Point>(nodes.map((n, i) => [n.id, [i * 100, i * 50]]));
        const square = { width: 600 + 2 * VIEW_PAD[0], height: 600 + 2 * VIEW_PAD[1] };
        const item = itemOf(starSeries(view({ positions: at, canvas: square })), SYS);
        expect([item.x, item.y]).toEqual([300, 150 * STRETCH_LIMIT]);
    });

    it('fills the canvas up to a margin that keeps the outer captions inside', () => {
        const series = starSeries(view());
        expect([series.left, series.right]).toEqual([VIEW_PAD[0], VIEW_PAD[0]]);
        expect([series.top, series.bottom]).toEqual([VIEW_PAD[1], VIEW_PAD[1]]);
        // Without this the chart stretches the map to the box and nodes turn oval.
        expect(series.preserveAspect).toBe('contain');
    });

    it('leaves out a relation that would land on a node caption or on another relation', () => {
        const [a, b, c] = [
            node('sys:demo/a', 'system'),
            node('sys:demo/b', 'system'),
            node('def:c', 'def'),
        ];
        const some = { nodes: [a, b, c], edges: [edge(a.id, 'calls', b.id)] };
        const at = (y: number) =>
            new Map<string, Point>([
                [a.id, [0, 0]],
                [b.id, [300, 0]],
                [c.id, [150, y]],
            ]);
        const big = { width: 300 + 2 * VIEW_PAD[0], height: 2000 };
        const shown = (y: number) =>
            starSeries(view({ ...some, positions: at(y), canvas: big })).links[0].label.show;
        // The caption of `c` hangs right where CALLS would be written.
        expect(shown(-30)).toBe(false);
        expect(shown(-150)).toBe(true);

        // Two edges that cross in their middle: only the first is written.
        const d = node('def:d', 'def');
        const cross = {
            nodes: [a, b, c, d],
            edges: [edge(a.id, 'calls', b.id), edge(c.id, 'covers', d.id)],
            positions: new Map<string, Point>([...at(-150), [d.id, [150, 150]]]),
            canvas: big,
        };
        expect(starSeries(view(cross)).links.map((l) => l.label.show)).toEqual([true, false]);
    });

    it('writes a relation only on an edge drawn long enough for it', () => {
        // Neighbours are 100 apart: OWNS fits between the rims, IMPLEMENTS does not.
        const series = starSeries(view());
        expect(linkOf(series, `${TEAM}|owns|${ENT}`).label.show).toBe(true);
        expect(linkOf(series, `${ENT}|implements|${SYS}`).label.show).toBe(false);
        // Drawn twice as large, both fit.
        const wide = { width: 1200 + 2 * VIEW_PAD[0], height: 120 + 2 * VIEW_PAD[1] };
        const large = starSeries(view({ canvas: wide }));
        expect(linkOf(large, `${ENT}|implements|${SYS}`).label.show).toBe(true);
        // Zooming in does the same.
        const zoomed = starSeries(view({}, { zoom: 2 }));
        expect(linkOf(zoomed, `${ENT}|implements|${SYS}`).label.show).toBe(true);
        // Selecting an end does not squeeze a caption onto a short edge.
        const picked = starSeries(view({}, { selected: SYS }));
        expect(linkOf(picked, `${ENT}|implements|${SYS}`).label.show).toBe(false);
    });

    it('writes every relation of a node on hover, also one that is left out at rest', () => {
        const series = starSeries(view());
        const short = linkOf(series, `${ENT}|implements|${SYS}`);
        expect(short.label.show).toBe(false);
        expect(short.emphasis).toEqual({ label: { show: true } });
    });

    it('gives the node caption a pill in the panel colour, so nothing under it shows', () => {
        const series = starSeries(view());
        expect(series.label.backgroundColor).toBe(
            chartTokens('tumult-dark').tooltip.backgroundColor,
        );
        expect(series.label.borderRadius).toBeGreaterThan(0);
        expect(series.label.padding).toEqual([1, 4]);
    });

    it('writes the kind under the name of two neighbours with the same name', () => {
        const wl = node('wl:demo/deployment/queue', 'workload', SYS);
        const series = starSeries(view({ nodes: [...nodes, wl] }));
        const text = (n: StarNode) => series.label.formatter({ data: { raw: n } });
        expect(text(nodes[3])).toBe('queue\n{kind|system}');
        expect(text(wl)).toBe('queue\n{kind|workload}');
        expect(text(nodes[2])).not.toContain('{kind|');
        expect(series.label.rich.kind.fontSize).toBeLessThan(series.label.fontSize);
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
        const focused = starSeries(view({ focused: true }, { zoom: 0.8 }));
        expect(linkOf(focused, `${TEAM}|owns|${ENT}`).label.show).toBe(true);
        expect(linkOf(starSeries(view({}, { zoom: 0.8 })), `${TEAM}|owns|${ENT}`).label.show).toBe(
            false,
        );
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
        // The test hangs below its system, clear of the row of the other nodes.
        const below = new Map(positions).set(DEF, [300, 230]);
        const high = { width: 600 + 2 * VIEW_PAD[0], height: 220 + 2 * VIEW_PAD[1] };
        const series = starSeries(
            view({ positions: below, canvas: high }, { selected: SYS, zoom: 0.95 }),
        );
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
        // The edges of the selected node keep their caption, zoomed out too,
        // where it fits.
        const own = linkOf(series, `${SYS}|tested_by|${DEF}`);
        expect(own.lineStyle.opacity).toBeGreaterThan(0.7);
        expect(own.label.show).toBe(true);
        expect(linkOf(series, `${ENT}|implements|${SYS}`).label.show).toBe(false);
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
