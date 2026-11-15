import { describe, expect, it } from 'vitest';
import { chartTokens } from '../echarts.js';
import type { ChartTheme } from '../echarts.js';
import type { StarEdge, StarNode } from '../types.js';
import {
    KINDS,
    KIND_COLOR,
    LENSES,
    RELS,
    REL_KEYS,
    caption,
    flagText,
    labelShown,
    labelText,
    relCaption,
    relName,
    relStroke,
    rollupLines,
    PART_OF,
    spineTooltipHtml,
    statusOf,
    toItem,
    toLink,
    toSpineLink,
    tooltipHtml,
} from './style.js';
import type { StyleContext } from './style.js';

const DAY_NS = 86_400_000_000_000;
const NOW_NS = 1_800_000_000 * 1_000_000_000;

function node(over: Partial<StarNode> = {}): StarNode {
    return {
        id: 'sys:demo/message-queue',
        kind: 'system',
        label: 'message-queue',
        parent: 'ent:cif-0003',
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
        ...over,
    };
}

const risks = (over: Partial<StarNode['risks']>): StarNode['risks'] => ({
    ...node().risks,
    ...over,
});

function ctx(over: Partial<StyleContext> = {}): StyleContext {
    return {
        lens: 'evidence',
        tokens: chartTokens('tumult-dark'),
        selected: null,
        zoom: 1,
        nowNs: NOW_NS,
        ...over,
    };
}

const t = chartTokens('tumult-dark');

function statusColours(theme: string): string[] {
    const tokens = chartTokens(theme as ChartTheme);
    return [tokens.ok, tokens.warn, tokens.fail];
}

describe('toItem', () => {
    it('names the node by id, puts it in its kind category and at its position', () => {
        const item = toItem(node(), [10, -20], ctx());
        expect(item.id).toBe('sys:demo/message-queue');
        expect(item.name).toBe('sys:demo/message-queue');
        expect(item.category).toBe('system');
        expect([item.x, item.y]).toEqual([10, -20]);
        expect(KINDS).toContain('system');
    });

    it('draws every kind as a circle large enough to find', () => {
        for (const kind of KINDS) {
            const item = toItem(node({ kind }), [0, 0], ctx());
            expect(item.symbol, kind).toBe('circle');
            expect(item.symbolSize, kind).toBeGreaterThanOrEqual(24);
        }
    });

    it('fills every kind with a colour of its own, whatever the lens', () => {
        expect(new Set(KINDS.map((kind) => KIND_COLOR[kind])).size).toBe(KINDS.length);
        for (const lens of LENSES) {
            const tested = node({ evidence: { ...node().evidence, runs: 3, failed: 1 } });
            expect(toItem(tested, [0, 0], ctx({ lens })).itemStyle.color).toBe(KIND_COLOR.system);
        }
    });

    it('keeps the fills clear of the status colours and of the yellow lost on white', () => {
        const reserved = ['#F0E442', ...['tumult-dark', 'tumult-light'].flatMap(statusColours)];
        for (const kind of KINDS) expect(reserved, kind).not.toContain(KIND_COLOR[kind]);
    });

    it('draws a missing-evidence node hollow and dashed', () => {
        const style = toItem(node({ gap: 'missing' }), [0, 0], ctx()).itemStyle;
        expect(style.color).toBe(t.tooltip.backgroundColor);
        expect(style.borderType).toBe('dashed');
        expect(style.borderColor).toBe(t.text);
        expect(style.borderWidth).toBe(2);
    });

    it('draws stale evidence with a warn dash but keeps the fill', () => {
        const style = toItem(node({ gap: 'stale' }), [0, 0], ctx()).itemStyle;
        expect(style.color).toBe(KIND_COLOR.system);
        expect(style.borderType).toEqual([2, 3]);
        expect(style.borderColor).toBe(t.warn);
    });

    it('dims a node whose risks are all suppressed and dots its ring', () => {
        const far = NOW_NS + 60 * DAY_NS;
        const style = toItem(
            node({ risks: risks({ suppressed: 2, suppression_expires_ns: far }) }),
            [0, 0],
            ctx(),
        ).itemStyle;
        expect(style.opacity).toBe(0.35);
        expect(style.borderColor).toBe(t.text);
        expect(style.borderType).toBe('dotted');
    });

    it('flags a suppression that expires within fourteen days', () => {
        const soon = NOW_NS + 3 * DAY_NS;
        const style = toItem(
            node({ risks: risks({ suppressed: 1, suppression_expires_ns: soon }) }),
            [0, 0],
            ctx(),
        ).itemStyle;
        expect(style.opacity).toBe(0.35);
        expect(style.borderColor).toBe(t.warn);
        expect(style.borderType).toBe('dotted');
    });

    it('does not dim a node that still has an open risk', () => {
        const style = toItem(
            node({ risks: risks({ open: 1, suppressed: 1, worst: 'high' }) }),
            [0, 0],
            ctx({ lens: 'risk' }),
        ).itemStyle;
        expect(style.opacity).toBeUndefined();
        expect(style.borderColor).toBe(t.fail);
    });

    it('rings a node by evidence in the evidence lens', () => {
        const ring = (runs: number, failed: number) =>
            toItem(node({ evidence: { ...node().evidence, runs, failed } }), [0, 0], ctx())
                .itemStyle;
        expect(ring(3, 1).borderColor).toBe(t.fail);
        expect(ring(3, 0).borderColor).toBe(t.ok);
        expect(ring(0, 0).borderColor).toBe(t.axis);
    });

    it('rings a node by its worst open risk in the risk lens', () => {
        const ring = (worst: string | null) =>
            toItem(
                node({ risks: risks({ open: worst ? 1 : 0, worst }) }),
                [0, 0],
                ctx({ lens: 'risk' }),
            ).itemStyle.borderColor;
        expect(ring('critical')).toBe(t.fail);
        expect(ring('high')).toBe(t.fail);
        expect(ring('medium')).toBe(t.warn);
        expect(ring('low')).toBe(t.warn);
        expect(ring(null)).toBe(t.axis);
    });

    it('rings services by conformance level in the conformance lens', () => {
        const ring = (conformance: string | null) =>
            toItem(node({ kind: 'service', conformance }), [0, 0], ctx({ lens: 'conformance' }))
                .itemStyle.borderColor;
        expect(ring('spine')).toBe(t.ok);
        expect(ring('target')).toBe(t.warn);
        expect(ring('none')).toBe(t.fail);
        expect(ring(null)).toBe(t.axis);
    });

    it('never says a status by colour alone: the ring also grows with it', () => {
        const width = (runs: number, failed: number) =>
            toItem(node({ evidence: { ...node().evidence, runs, failed } }), [0, 0], ctx())
                .itemStyle.borderWidth ?? 0;
        expect(width(3, 1)).toBeGreaterThan(width(3, 0));
        expect(width(3, 0)).toBeGreaterThan(width(0, 0));
    });

    it('marks the selected node with a wide accent ring and a glow', () => {
        const style = toItem(node(), [0, 0], ctx({ selected: 'sys:demo/message-queue' })).itemStyle;
        expect(style.borderColor).toBe(t.accent);
        expect(style.borderWidth).toBe(5);
        expect(style.shadowColor).toBe(t.accent);
    });

    it('takes every colour from the theme in force', () => {
        const light = chartTokens('tumult-light');
        const style = toItem(node({ gap: 'missing' }), [0, 0], ctx({ tokens: light })).itemStyle;
        expect(style.borderColor).toBe(light.text);
    });
});

describe('statusOf and caption', () => {
    const failed = node({ evidence: { ...node().evidence, runs: 2, failed: 1 } });

    it('names the state of a node under each lens', () => {
        expect(statusOf(failed, 'evidence')).toBe('fail');
        expect(statusOf(failed, 'risk')).toBeNull();
        expect(statusOf(node({ risks: risks({ open: 1, worst: 'low' }) }), 'risk')).toBe('warn');
        expect(statusOf(node({ conformance: 'spine' }), 'conformance')).toBe('ok');
    });

    it('puts a mark for the state before the name', () => {
        expect(caption(failed, 'evidence')).toBe('✕ message-queue');
        expect(caption(node(), 'evidence')).toBe('message-queue');
        const passed = node({ evidence: { ...node().evidence, runs: 2 } });
        expect(caption(passed, 'evidence')).toBe('✓ message-queue');
        expect(caption(node({ risks: risks({ open: 1, worst: 'low' }) }), 'risk')).toBe(
            '! message-queue',
        );
    });

    it('cuts a long name short with an ellipsis', () => {
        const long = node({ label: 'queue-dependency-loss-with-restart' });
        expect(caption(long, 'evidence')).toBe('queue-dependenc…');
        expect(caption(node({ label: 'exactly-16-chars' }), 'evidence')).toBe('exactly-16-chars');
        // A map drawn small leaves less room under a node.
        expect(caption(long, 'evidence', 8)).toBe('queue-d…');
    });

    it('keeps a name from being read as ECharts rich text', () => {
        expect(caption(node({ label: '{a|b}' }), 'evidence')).toBe('{​a|b}');
    });
});

describe('labelShown', () => {
    it('labels everything from zoom 1 and only the large kinds below it', () => {
        expect(labelShown('org', 0)).toBe(true);
        expect(labelShown('entity', 0)).toBe(true);
        expect(labelShown('system', 0)).toBe(true);
        expect(labelShown('cluster', 0)).toBe(true);
        expect(labelShown('workload', 0)).toBe(false);
        expect(labelShown('def', 0)).toBe(false);
        for (const kind of KINDS) expect(labelShown(kind, 1), kind).toBe(true);
    });

    it('is applied to the item, except that a selected node is always labelled', () => {
        const service = node({ id: 'svc:a', kind: 'service' });
        expect(toItem(service, [0, 0], ctx({ zoom: 0 })).label.show).toBe(false);
        expect(toItem(service, [0, 0], ctx()).label.show).toBe(true);
        expect(toItem(service, [0, 0], ctx({ zoom: 0, selected: 'svc:a' })).label.show).toBe(true);
    });
});

describe('toLink', () => {
    const edge = (rel: StarEdge['rel'], over: Partial<StarEdge> = {}): StarEdge => ({
        id: `a|${rel}|b`,
        from: 'a',
        to: 'b',
        rel,
        source: 'declared',
        ...over,
    });

    it('draws every relation as a visible arrow from its source to its target', () => {
        for (const rel of RELS) {
            const link = toLink(edge(rel), ctx());
            expect([link.source, link.target], rel).toEqual(['a', 'b']);
            expect(link.symbol, rel).toEqual(['none', 'arrow']);
            expect(link.lineStyle.width, rel).toBeGreaterThanOrEqual(1.2);
            expect(link.lineStyle.opacity, rel).toBeGreaterThanOrEqual(0.7);
        }
    });

    it('writes the relation the API named along the edge, in upper case', () => {
        expect(RELS.map(relCaption)).toEqual([
            'OWNS',
            'IMPLEMENTS',
            'CALLS',
            'RUNS_ON',
            'DEPENDS_ON',
            'TESTED_BY',
            'BROKE',
            'COVERS',
            'MEASURES',
        ]);
        expect(toLink(edge('runs_on'), ctx()).label.formatter).toBe('RUNS_ON');
    });

    it('draws every relation with the stroke of its kind, and says which it is', () => {
        for (const rel of RELS) {
            const link = toLink(edge(rel), ctx());
            expect(link.rel, rel).toBe(rel);
            expect(link.lineStyle, rel).toEqual(relStroke(rel, t));
        }
    });

    it('widens a call edge with its traffic, never to the width of a broke edge', () => {
        const width = (weight: number) => toLink(edge('calls', { weight }), ctx()).lineStyle.width;
        expect(width(1000)).toBeGreaterThan(width(1));
        expect(width(1e12)).toBeLessThan(relStroke('broke', t).width);
    });

    it('draws what a run broke red and thick', () => {
        const stroke = toLink(edge('broke'), ctx()).lineStyle;
        expect(stroke.color).toBe(t.fail);
        expect(stroke.type).toBe('solid');
        for (const rel of RELS.filter((r) => r !== 'broke')) {
            expect(stroke.width, rel).toBeGreaterThan(relStroke(rel, t).width);
        }
    });
});

describe('relStroke', () => {
    const themes: ChartTheme[] = ['tumult-dark', 'tumult-light'];

    function luminance(hex: string): number {
        const channel = (i: number) => {
            const v = parseInt(hex.slice(1 + 2 * i, 3 + 2 * i), 16) / 255;
            return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
        };
        return 0.2126 * channel(0) + 0.7152 * channel(1) + 0.0722 * channel(2);
    }

    function contrast(a: string, b: string): number {
        const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
        return (hi + 0.05) / (lo + 0.05);
    }

    it('names PART_OF next to the nine relations of the API', () => {
        expect(REL_KEYS).toEqual([...RELS, 'part_of']);
        expect(relName('part_of')).toBe(PART_OF);
        expect(relName('runs_on')).toBe('RUNS_ON');
    });

    it('gives every relation its own colour on both themes', () => {
        for (const theme of themes) {
            const colors = REL_KEYS.map((rel) => relStroke(rel, chartTokens(theme)).color);
            expect(new Set(colors).size, theme).toBe(REL_KEYS.length);
        }
    });

    it('gives every relation its own dash, or for a solid line its own width', () => {
        const shape = (rel: (typeof REL_KEYS)[number]) => {
            const stroke = relStroke(rel, t);
            return stroke.type === 'solid' ? `solid ${stroke.width}` : stroke.type.join(' ');
        };
        expect(new Set(REL_KEYS.map(shape)).size).toBe(REL_KEYS.length);
        const solid = REL_KEYS.filter((rel) => relStroke(rel, t).type === 'solid');
        expect(solid).toEqual(['calls', 'broke', 'part_of']);
    });

    it('draws every relation in a colour that stands out on both themes', () => {
        for (const theme of themes) {
            const tokens = chartTokens(theme);
            for (const rel of REL_KEYS) {
                const ratio = contrast(
                    relStroke(rel, tokens).color,
                    tokens.tooltip.backgroundColor,
                );
                expect(ratio, `${rel} on ${theme}`).toBeGreaterThanOrEqual(3);
            }
        }
    });

    it('keeps PART_OF thin and grey, so that it reads as structure', () => {
        const stroke = relStroke('part_of', t);
        expect(stroke).toEqual({ color: t.text, width: 1, type: 'solid', opacity: 0.55 });
    });
});

describe('toSpineLink', () => {
    it('draws the parent of a node as a thin grey arrow from the child, named PART_OF', () => {
        const link = toSpineLink('cmp:demo/mq/broker', 'sys:demo/mq', ctx());
        expect(link.id).toBe('spine|cmp:demo/mq/broker');
        expect([link.source, link.target]).toEqual(['cmp:demo/mq/broker', 'sys:demo/mq']);
        expect(link.symbol).toEqual(['none', 'arrow']);
        expect(link.label).toEqual({ show: true, formatter: PART_OF });
        expect(link.rel).toBe('part_of');
        expect(PART_OF).toBe('PART_OF');
        expect(link.lineStyle.color).toBe(t.text);
        expect(link.lineStyle.width).toBeLessThan(1.2);
        expect(link.lineStyle.opacity).toBeLessThan(0.7);
        expect(link.raw).toBeNull();
    });

    it('says in its tooltip which node is part of which, as text', () => {
        const html = spineTooltipHtml('cmp:<b>', 'sys:demo/mq').html;
        expect(html).toContain('cmp:&lt;b&gt; part of sys:demo/mq');
        expect(html).toContain('the parent of the node');
    });
});

describe('tooltipHtml', () => {
    it('names the score of a node resilience, not posture', () => {
        const { html } = tooltipHtml(node({ posture: { score: 41.5, band: 'poor' } }), ctx());
        expect(html).toContain('resilience: 41.5 (poor)');
        expect(html).not.toContain('posture');
    });

    it('escapes names that come from telemetry', () => {
        const { html } = tooltipHtml(node({ label: '<script>alert(1)</script>' }), ctx());
        expect(html).not.toContain('<script>');
        expect(html).toContain('&lt;script&gt;');
    });

    it('says who suppressed a risk, until when and why', () => {
        const { html } = tooltipHtml(
            node({
                risks: risks({
                    suppressed: 1,
                    suppression_expires_ns: Date.UTC(2027, 0, 15) * 1_000_000,
                    suppressed_by: 'morgan<b>',
                    suppression_reason: 'accepted until the <new> broker ships',
                }),
            }),
            ctx(),
        );
        expect(html).toContain(
            'suppressed by morgan&lt;b&gt; until 2027-01-15: accepted until the &lt;new&gt; broker ships',
        );
    });

    it('says so when a suppression never expires', () => {
        const { html } = tooltipHtml(
            node({
                risks: risks({ suppressed: 1, suppressed_by: 'ops', suppression_reason: 'known' }),
            }),
            ctx(),
        );
        expect(html).toContain('suppressed by ops with no expiry: known');
    });

    it('names the missing disciplines and the recommended kind on a gap', () => {
        const { html } = tooltipHtml(
            node({
                gap: 'missing',
                evidence: {
                    ...node().evidence,
                    by_discipline: { chaos: 'missing', dr: 'missing', load: 'covered' },
                },
                risks: risks({ open: 1, worst: 'high', recommended: ['dr'] }),
            }),
            ctx(),
        );
        expect(html).toContain('no evidence: chaos, dr');
        expect(html).toContain('recommended: dr');
    });

    it('reports the conformance level in the conformance lens', () => {
        const { html } = tooltipHtml(
            node({ kind: 'service', conformance: 'target' }),
            ctx({ lens: 'conformance' }),
        );
        expect(html).toContain('conformance: target');
    });

    it('counts open and suppressed risks separately', () => {
        const { html } = tooltipHtml(
            node({ risks: risks({ open: 2, suppressed: 1, worst: 'medium' }) }),
            ctx({ lens: 'risk' }),
        );
        expect(html).toContain('2 open');
        expect(html).toContain('1 suppressed');
    });
});

describe('labelText', () => {
    const RICH = /\{[a-zA-Z0-9_]+\|[^}]*\}/;

    it('leaves an ordinary label alone', () => {
        expect(labelText(node())).toBe('message-queue');
    });

    it('keeps a label from being read as ECharts rich text', () => {
        const text = labelText(node({ label: 'pay{a|ments} {b}' }));
        expect(text).not.toMatch(RICH);
        expect(text.replace(/\u200b/g, '')).toBe('pay{a|ments} {b}');
    });
});

describe('flags and roll-ups', () => {
    const rollup = {
        systems: 6,
        untested_systems: 5,
        stale_systems: 0,
        failed: 2,
        risks_open: 3,
        risks_suppressed: 1,
        gaps: 5,
    };

    it('has a sentence for every flag and passes an unknown one through', () => {
        expect(flagText('malformed_id')).toContain('agreed form');
        expect(flagText('multiple_owners')).toContain('more than one owner');
        expect(flagText('owner_conflict')).toContain('org.yaml');
        expect(flagText('later_flag')).toBe('later_flag');
    });

    it('says how many systems an entity hides behind its best one', () => {
        expect(rollupLines(node({ kind: 'entity', rollup }))).toEqual([
            '5 of 6 systems untested',
            'below: 5 gaps, 2 failed runs, 3 open risks, 1 suppressed',
        ]);
        expect(rollupLines(node({ kind: 'entity', rollup: null }))).toEqual([]);
    });

    it('mentions stale systems only when there are some', () => {
        const lines = rollupLines(
            node({ rollup: { ...rollup, untested_systems: 0, stale_systems: 2 } }),
        );
        expect(lines[0]).toBe('0 of 6 systems untested, 2 stale');
    });

    it('puts flags and the roll-up in the tooltip, escaped', () => {
        const { html } = tooltipHtml(
            node({ kind: 'entity', flags: ['malformed_id', '<b>'], rollup }),
            ctx(),
        );
        expect(html).toContain('agreed form');
        expect(html).toContain('&lt;b&gt;');
        expect(html).toContain('5 of 6 systems untested');
    });
});
