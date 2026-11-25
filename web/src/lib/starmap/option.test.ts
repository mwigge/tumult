import { describe, expect, it } from 'vitest';
import { chartTokens } from '../echarts.js';
import type { StarEdge, StarKind, StarNode, StarRel } from '../types.js';
import type { Point } from './layout.js';
import {
    CAPTION_LIMIT,
    CAPTION_SPOTS,
    STRETCH_LIMIT,
    VIEW_PAD,
    captionAnchor,
    captionBox,
    captionFits,
    captionShares,
    captionSpot,
    captionsShown,
    fillCanvas,
    fitScale,
    overlaps,
    legendKinds,
    legendRels,
    linksAt,
    nameLimit,
    spinePairs,
    starSeries,
    statusKey,
    tipPlace,
    twins,
} from './option.js';
import type { StarView } from './option.js';
import { KIND_COLOR, relStroke } from './style.js';
import type { RelKey } from './style.js';

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
        hiddenRels: new Set(),
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

describe('captionBox along the edge', () => {
    it('moves with the share of the drawn line it is written at', () => {
        const mid = captionBox('OWNS', [0, 0], [200, 0], 0);
        expect(captionBox('OWNS', [0, 0], [200, 0], 0, [0, 0], 0.35).x).toBe(70);
        expect(captionBox('OWNS', [0, 0], [200, 0], 0, [0, 0], 0.65).x).toBe(130);
        expect(captionBox('OWNS', [0, 0], [200, 0], 0, [0, 0], 0.35).y).toBe(mid.y);
        // The line runs from 10 to 170: 0.25 of it is at 50.
        expect(captionBox('OWNS', [0, 0], [200, 0], 0, [20, 60], 0.25).x).toBe(50);
        // From the far end the shares count the other way.
        expect(captionBox('OWNS', [200, 0], [0, 0], 0, [0, 0], 0.35).x).toBe(130);
    });
});

describe('captionSpot', () => {
    const line = { text: 'OWNS', a: [0, 0] as Point, b: [200, 0] as Point, sizes: [0, 0] as Point };
    const boxAt = (at: number, below = false) =>
        captionBox('OWNS', line.a, line.b, 0, line.sizes, at, below);
    /** Every place along the line, on both sides of it. */
    const both = (shares: number[]) => shares.flatMap((at) => [boxAt(at), boxAt(at, true)]);

    it('tries the middle first, then towards either end', () => {
        expect(CAPTION_SPOTS[0]).toBe(0.5);
        expect(CAPTION_SPOTS).toContain(0.35);
        expect(CAPTION_SPOTS).toContain(0.65);
    });

    it('tries last the two places where the text ends just short of a rim', () => {
        // OWNS is 22 wide, with 3 of air: its middle is 14 from the rim.
        expect(captionShares('OWNS', 200)).toEqual([...CAPTION_SPOTS, 0.07, 0.93]);
        expect(captionFits('OWNS', [0, 0], [200, 0], 0, 0, 1, 0.07)).toBe(true);
        // On a short line they come nearer the middle; on one too short, none is added.
        expect(captionShares('OWNS', 35)).toEqual([...CAPTION_SPOTS, 0.4, 0.6]);
        expect(captionShares('OWNS', 28)).toEqual(CAPTION_SPOTS);
    });

    it('is the middle of the edge, above the line, when that is free', () => {
        expect(captionSpot({ ...line, bend: 0 }, [])).toEqual({ at: 0.5, below: false });
    });

    it('writes the text below the line when above it is taken', () => {
        expect(captionSpot({ ...line, bend: 0 }, [boxAt(0.5)])).toEqual({ at: 0.5, below: true });
    });

    it('is the next free place along the edge when the middle is taken', () => {
        const next = { at: CAPTION_SPOTS[1], below: false };
        expect(captionSpot({ ...line, bend: 0 }, both([0.5]))).toEqual(next);
        const two = both([0.5, CAPTION_SPOTS[1]]);
        expect(captionSpot({ ...line, bend: 0 }, two)?.at).toBe(CAPTION_SPOTS[2]);
    });

    it('writes the text at the free end of an edge whose start is taken', () => {
        const taken = both([...CAPTION_SPOTS, 0.07]);
        expect(captionSpot({ ...line, bend: 0 }, taken)?.at).toBe(0.93);
    });

    it('is null when every place is taken', () => {
        const every = both(captionShares('OWNS', 200));
        expect(captionSpot({ ...line, bend: 0 }, every)).toBeNull();
    });

    it('is null on an edge too short for the text, and skips a place too near a rim', () => {
        const short = { ...line, b: [25, 0] as Point, bend: 0 };
        expect(captionSpot(short, [])).toBeNull();
        // 36 long: OWNS (22 wide) fits in the middle, not nearer an end.
        const tight = { ...line, b: [36, 0] as Point, bend: 0 };
        expect(captionSpot(tight, [])?.at).toBe(0.5);
        const mid = [false, true].map((below) =>
            captionBox('OWNS', tight.a, tight.b, 0, [0, 0], 0.5, below),
        );
        expect(captionSpot(tight, mid)).toBeNull();
    });

    it('tries only the middle of a bent edge', () => {
        const bent = { ...line, bend: 0.2 };
        expect(captionSpot(bent, [])).toEqual({ at: 0.5, below: false });
        const mid = [false, true].map((below) =>
            captionBox('OWNS', line.a, line.b, 0.2, [0, 0], 0.5, below),
        );
        expect(captionSpot(bent, mid)).toBeNull();
    });
});

describe('captionBox below the line', () => {
    it('is as far below the line as the caption above it is above', () => {
        const above = captionBox('OWNS', [0, 0], [200, 0], 0);
        const below = captionBox('OWNS', [0, 0], [200, 0], 0, [0, 0], 0.5, true);
        expect([below.x, below.y]).toEqual([above.x, -above.y]);
    });
});

describe('captionAnchor', () => {
    // The chart starts a line at the centre of its source, 10 inside the rim
    // of this one, ends it at the rim of its target, and centres the text.
    const line = { drawn: 200, rim: 10, bent: false, scale: 1 };

    it('measures a caption from the centre of the source to the middle of the text', () => {
        expect(captionAnchor({ at: 0.5, below: false }, line)).toEqual({
            position: 'insideStartTop',
            distance: [110, 5],
        });
        expect(captionAnchor({ at: 0.35, below: true }, line)).toEqual({
            position: 'insideStartBottom',
            distance: [80, 5],
        });
    });

    it('measures a caption nearer the target from the end of the line', () => {
        expect(captionAnchor({ at: 0.65, below: false }, line)).toEqual({
            position: 'insideEndTop',
            distance: [70, 5],
        });
    });

    it('gives the distances in the units of the map, which the chart draws at its scale', () => {
        const half = { ...line, scale: 0.5 };
        expect(captionAnchor({ at: 0.5, below: false }, half)).toEqual({
            position: 'insideStartTop',
            distance: [55, 2.5],
        });
    });

    it('leaves a caption on a bent edge to the chart, in the middle of the curve', () => {
        const bent = { ...line, bent: true };
        expect(captionAnchor({ at: 0.5, below: false }, bent)).toEqual({
            position: 'middle',
        });
        expect(captionAnchor({ at: 0.5, below: true }, bent)).toEqual({
            position: 'insideMiddleBottom',
            distance: [0, 5],
        });
    });
});

describe('tipPlace', () => {
    const view: Point = [1000, 800];
    const tip: Point = [200, 100];

    it('puts the tooltip above the node, so the caption below it stays in sight', () => {
        const [x, y] = tipPlace({ x: 480, y: 400, width: 40, height: 40 }, tip, view);
        expect(x).toBe(400);
        expect(y + tip[1]).toBeLessThan(400);
    });

    it('keeps it inside the chart at the sides', () => {
        expect(tipPlace({ x: 10, y: 400, width: 40, height: 40 }, tip, view)[0]).toBe(0);
        expect(tipPlace({ x: 950, y: 400, width: 40, height: 40 }, tip, view)[0]).toBe(800);
    });

    it('puts it beside a node at the top, clear of the caption under the node', () => {
        const [x, y] = tipPlace({ x: 480, y: 20, width: 40, height: 40 }, tip, view);
        expect(y).toBe(0);
        expect(x).toBeGreaterThan(500 + 50);
        // Without room on the right it goes to the left.
        const [left] = tipPlace({ x: 900, y: 20, width: 40, height: 40 }, tip, view);
        expect(left + tip[0]).toBeLessThan(920 - 50);
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

    it('takes a turned box as it is drawn, not as the square around it', () => {
        // A thin box at 45 degrees from the origin, and one in its corner.
        const s = Math.SQRT1_2;
        const turned = {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
            turn: { u: [s, s] as Point, half: [20, 2] as Point },
        };
        const corner = { x: 12, y: -12, w: 3, h: 3 };
        expect(overlaps(turned, [corner])).toBe(false);
        expect(overlaps(corner, [turned])).toBe(false);
        expect(overlaps(turned, [{ x: 10, y: 10, w: 3, h: 3 }])).toBe(true);
    });

    it('is the box of a caption on a slanted edge turned along that edge', () => {
        const slant = captionBox('OWNS', [0, 0], [100, 100], 0);
        expect(slant.turn?.half).toEqual([11, 8]);
        expect(slant.turn?.u[0]).toBeCloseTo(Math.SQRT1_2);
        expect(slant.turn?.u[1]).toBeCloseTo(Math.SQRT1_2);
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

describe('linksAt', () => {
    const links = [
        { source: 'a', target: 'b' },
        { source: 'b', target: 'c' },
        { source: 'c', target: 'a' },
        { source: 'c', target: 'd' },
    ];

    it('is the index of every link that starts or ends at the node', () => {
        expect(linksAt(links, 'a')).toEqual([0, 2]);
        expect(linksAt(links, 'd')).toEqual([3]);
        expect(linksAt(links, 'x')).toEqual([]);
    });
});

describe('legendRels', () => {
    const t = chartTokens('tumult-dark');

    it('lists the relations on the map with their line and count, PART_OF last', () => {
        expect(legendRels(nodes, edges, t)).toEqual([
            { rel: 'owns', name: 'OWNS', count: 1, stroke: relStroke('owns', t) },
            { rel: 'implements', name: 'IMPLEMENTS', count: 1, stroke: relStroke('implements', t) },
            { rel: 'tested_by', name: 'TESTED_BY', count: 1, stroke: relStroke('tested_by', t) },
            { rel: 'broke', name: 'BROKE', count: 1, stroke: relStroke('broke', t) },
            { rel: 'part_of', name: 'PART_OF', count: 2, stroke: relStroke('part_of', t) },
        ]);
    });

    it('leaves out an edge that has an end off the map', () => {
        const some = [...edges, edge(SYS, 'calls', 'sys:demo/gone')];
        expect(legendRels(nodes, some, t).map((r) => r.rel)).not.toContain('calls');
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
        // The mono font of the pages, ending in the generic monospace.
        expect(series.edgeLabel.fontFamily).toBe(chartTokens('tumult-dark').font.mono);
        expect(series.edgeLabel.fontFamily).toMatch(/, monospace$/);
        // A caption is centred on the place it is measured to.
        expect(series.edgeLabel.align).toBe('center');
        const part = linkOf(series, `spine|${CMP}`);
        expect([part.source, part.target]).toEqual([CMP, SYS]);
        expect(part.symbol).toEqual(['none', 'arrow']);
        expect(part.label).toMatchObject({ show: true, formatter: 'PART_OF' });
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
            node('def:name/a-very-long-name', 'def'),
        ];
        const some = { nodes: [a, b, c], edges: [edge(a.id, 'calls', b.id)] };
        const at = (y: number) =>
            new Map<string, Point>([
                [a.id, [0, 0]],
                [b.id, [150, 0]],
                [c.id, [75, y]],
            ]);
        const big = { width: 150 + 2 * VIEW_PAD[0], height: 2000 };
        const shown = (y: number) =>
            starSeries(view({ ...some, positions: at(y), canvas: big })).links[0].label.show;
        // The caption of `c` hangs over every place where CALLS would be written.
        expect(shown(-16)).toBe(false);
        expect(shown(-150)).toBe(true);

        // Two edges that cross in their middle: the second is not written there.
        const d = node('def:d', 'def');
        const cross = {
            nodes: [a, b, c, d],
            edges: [edge(a.id, 'calls', b.id), edge(c.id, 'covers', d.id)],
            positions: new Map<string, Point>([...at(-150), [d.id, [75, 150]]]),
            canvas: big,
        };
        const labels = starSeries(view(cross)).links.map((l) => l.label);
        expect(labels[0]).toMatchObject({ show: true, position: 'insideStartTop' });
        expect(labels[1]).not.toEqual(labels[0]);
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
    });

    it('counts the kind line under a name as the small line it is', () => {
        const [a, b] = [node('sys:demo/a', 'system'), node('sys:demo/b', 'system')];
        const [sys, wl] = [node('sys:demo/q', 'system'), node('wl:demo/q', 'workload')];
        const series = starSeries(
            view({
                nodes: [a, b, sys, wl],
                edges: [edge(a.id, 'calls', b.id), edge(sys.id, 'runs_on', wl.id)],
                positions: new Map<string, Point>([
                    [a.id, [-60, 60]],
                    [b.id, [120, 60]],
                    [wl.id, [30, 0]],
                    [sys.id, [400, -200]],
                ]),
                canvas: { width: 400 + 2 * VIEW_PAD[0], height: 200 + 2 * VIEW_PAD[1] },
            }),
        );
        // CALLS fits under `q` and its kind line, which is less than two names tall.
        expect(series.label.formatter({ data: { raw: wl } })).toBe('q\n{kind|workload}');
        expect(linkOf(series, `${a.id}|calls|${b.id}`).label.show).toBe(true);
    });

    it('writes a relation nearer one end when the middle of the edge is taken', () => {
        const [a, b, c] = [
            node('sys:demo/a', 'system'),
            node('sys:demo/b', 'system'),
            node('def:c', 'def'),
        ];
        // `c` sits on the middle of the long edge from `a` to `b`.
        const some = {
            nodes: [a, b, c],
            edges: [edge(a.id, 'calls', b.id)],
            positions: new Map<string, Point>([
                [a.id, [0, 0]],
                [b.id, [600, 0]],
                [c.id, [300, 0]],
            ]),
            canvas: { width: 600 + 2 * VIEW_PAD[0], height: 2000 },
        };
        const label = starSeries(view(some)).links[0].label;
        expect(label.show).toBe(true);
        expect(label.position).toBe('insideStartTop');
        expect(label.distance?.[0]).toBeGreaterThan(100);
    });

    it('writes every relation of the selected node, also where it would not fit at rest', () => {
        const rest = starSeries(view());
        expect(linkOf(rest, `${ENT}|implements|${SYS}`).label.show).toBe(false);
        const picked = starSeries(view({}, { selected: SYS }));
        for (const link of picked.links) {
            const own = link.source === SYS || link.target === SYS;
            expect(link.label.show, link.id).toBe(own);
        }
        // In focus mode too, where nothing is dimmed.
        const focused = starSeries(view({ focused: true }, { selected: SYS }));
        expect(linkOf(focused, `${ENT}|implements|${SYS}`).label.show).toBe(true);
    });

    it('keeps the names on the links of the selected node apart from each other', () => {
        const [s, a, b] = [
            node('sys:demo/s', 'system'),
            node('def:a', 'def'),
            node('def:b', 'def'),
        ];
        // Nodes with long names sit over and under both links: no room at rest.
        const blocker = node('def:name/a-very-long-name', 'def');
        const under = node('def:name/another-long-name', 'def');
        const series = starSeries(
            view(
                {
                    nodes: [s, a, b, blocker, under],
                    edges: [edge(s.id, 'tested_by', a.id), edge(s.id, 'covers', b.id)],
                    positions: new Map<string, Point>([
                        [s.id, [0, 0]],
                        [a.id, [92, 0]],
                        [b.id, [92, 8]],
                        [blocker.id, [46, -28]],
                        [under.id, [46, 22]],
                    ]),
                    canvas: { width: 92 + 2 * VIEW_PAD[0], height: 2000 },
                },
                { selected: s.id },
            ),
        );
        const [one, two] = series.links.map((l) => l.label);
        expect(one.show && two.show).toBe(true);
        // The second goes elsewhere along its line, or to its other side.
        const place = (l: typeof one) => [l.position, Math.round((l.distance?.[0] ?? 0) / 20)];
        expect(place(two)).not.toEqual(place(one));
    });

    it('leaves out the links of a relation switched off in the legend', () => {
        const off = new Set<RelKey>(['broke', 'part_of']);
        const series = starSeries(view({ hiddenRels: off }));
        expect(series.links.map((l) => l.id)).toEqual([
            `${TEAM}|owns|${ENT}`,
            `${ENT}|implements|${SYS}`,
            `${SYS}|tested_by|${DEF}`,
        ]);
        // The nodes stay, and the edge left of a pair runs straight again.
        expect(series.data).toHaveLength(nodes.length);
        expect(linkOf(series, `${SYS}|tested_by|${DEF}`).lineStyle.curveness).toBe(0);
    });

    it('keeps the caption of a faded node opaque, so no line shows through it', () => {
        expect(starSeries(view()).label.opacity).toBe(1);
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
        // The edges of the selected node keep their caption, zoomed out too.
        const own = linkOf(series, `${SYS}|tested_by|${DEF}`);
        expect(own.lineStyle.opacity).toBeGreaterThan(0.7);
        expect(own.label.show).toBe(true);
        expect(linkOf(series, `${ENT}|implements|${SYS}`).label.show).toBe(true);
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
