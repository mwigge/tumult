import { describe, expect, it } from 'vitest';
import { chartTokens, PALETTE } from '../echarts.js';
import type { StarEdge, StarNode } from '../types.js';
import {
    KINDS,
    esc,
    flagText,
    labelShown,
    labelText,
    rollupLines,
    toItem,
    toLink,
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

describe('esc', () => {
    it('escapes everything HTML gives meaning to', () => {
        expect(esc(`<img src=x onerror="a('b')">&`)).toBe(
            '&lt;img src=x onerror=&quot;a(&#39;b&#39;)&quot;&gt;&amp;',
        );
    });
});

describe('toItem', () => {
    it('names the node by id, puts it in its kind category and at its position', () => {
        const item = toItem(node(), [10, -20], ctx());
        expect(item.id).toBe('sys:demo/message-queue');
        expect(item.name).toBe('sys:demo/message-queue');
        expect(item.category).toBe('system');
        expect([item.x, item.y]).toEqual([10, -20]);
        expect(item.symbol).toBe('rect');
        expect(KINDS).toContain('system');
    });

    it('gives every kind its own shape', () => {
        const shape = (kind: StarNode['kind']) => toItem(node({ kind }), [0, 0], ctx()).symbol;
        expect(shape('org')).toBe('roundRect');
        expect(shape('entity')).toBe('diamond');
        expect(shape('service')).toBe('circle');
        expect(shape('workload')).toBe('triangle');
        expect(shape('ext')).toBe('pin');
        expect(shape('run')).toBe('emptyCircle');
        expect(shape('slo')).toBe('arrow');
        expect(shape('article')).toBe('rect');
    });

    it('draws a missing-evidence node hollow and dashed', () => {
        const style = toItem(node({ gap: 'missing' }), [0, 0], ctx()).itemStyle;
        expect(style.color).toBe('transparent');
        expect(style.borderType).toBe('dashed');
        expect(style.borderColor).toBe(t.text);
        expect(style.borderWidth).toBe(1.5);
    });

    it('draws stale evidence with a thin warn dash but keeps the fill', () => {
        const style = toItem(node({ gap: 'stale' }), [0, 0], ctx()).itemStyle;
        expect(style.color).not.toBe('transparent');
        expect(style.borderType).toEqual([2, 3]);
        expect(style.borderColor).toBe(t.warn);
    });

    it('dims a node whose risks are all suppressed', () => {
        const far = NOW_NS + 60 * DAY_NS;
        const style = toItem(
            node({ risks: risks({ suppressed: 2, suppression_expires_ns: far }) }),
            [0, 0],
            ctx(),
        ).itemStyle;
        expect(style.opacity).toBe(0.35);
        expect(style.borderColor).toBe(t.axis);
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
            ctx(),
        ).itemStyle;
        expect(style.opacity).toBeUndefined();
        expect(style.borderColor).toBe(t.fail);
    });

    it('borders open risks by severity', () => {
        const border = (worst: string) =>
            toItem(node({ risks: risks({ open: 1, worst }) }), [0, 0], ctx()).itemStyle.borderColor;
        expect(border('critical')).toBe(t.fail);
        expect(border('high')).toBe(t.fail);
        expect(border('medium')).toBe(t.warn);
        expect(border('low')).toBe(t.warn);
    });

    it('fills by evidence in the evidence lens', () => {
        const fill = (runs: number, failed: number) =>
            toItem(node({ evidence: { ...node().evidence, runs, failed } }), [0, 0], ctx())
                .itemStyle.color;
        expect(fill(3, 1)).toBe(t.fail);
        expect(fill(3, 0)).toBe(t.ok);
        expect(PALETTE).toContain(fill(0, 0));
    });

    it('fills by worst open risk in the risk lens', () => {
        const fill = (worst: string | null) =>
            toItem(
                node({ risks: risks({ open: worst ? 1 : 0, worst }) }),
                [0, 0],
                ctx({ lens: 'risk' }),
            ).itemStyle.color;
        expect(fill('critical')).toBe(t.fail);
        expect(fill('low')).toBe(t.warn);
        expect(PALETTE).toContain(fill(null));
    });

    it('fills services by conformance level in the conformance lens', () => {
        const fill = (conformance: string | null) =>
            toItem(node({ kind: 'service', conformance }), [0, 0], ctx({ lens: 'conformance' }))
                .itemStyle.color;
        expect(fill('spine')).toBe(t.ok);
        expect(fill('target')).toBe(t.warn);
        expect(fill('none')).toBe(t.fail);
        expect(PALETTE).toContain(fill(null));
    });

    it('never uses the yellow that vanishes on white', () => {
        for (const kind of KINDS) {
            const fill = toItem(node({ kind }), [0, 0], ctx()).itemStyle.color;
            expect(fill, kind).not.toBe('#F0E442');
        }
    });

    it('marks the selected node with the accent border', () => {
        const style = toItem(node(), [0, 0], ctx({ selected: 'sys:demo/message-queue' })).itemStyle;
        expect(style.borderColor).toBe(t.accent);
        expect(style.borderWidth).toBe(3);
    });

    it('takes every colour from the theme in force', () => {
        const light = chartTokens('tumult-light');
        const style = toItem(node({ gap: 'missing' }), [0, 0], ctx({ tokens: light })).itemStyle;
        expect(style.borderColor).toBe(light.text);
    });
});

describe('labelShown', () => {
    it('labels the inner rings always and the outer rings as the zoom grows', () => {
        expect(labelShown('org', 0.3)).toBe(true);
        expect(labelShown('entity', 0.3)).toBe(true);
        expect(labelShown('system', 0.9)).toBe(false);
        expect(labelShown('system', 1)).toBe(true);
        expect(labelShown('service', 1.9)).toBe(false);
        expect(labelShown('workload', 2)).toBe(true);
        expect(labelShown('cluster', 0.3)).toBe(true);
    });

    it('is applied to the item, except that a selected node is always labelled', () => {
        const service = node({ id: 'svc:a', kind: 'service' });
        expect(toItem(service, [0, 0], ctx()).label.show).toBe(false);
        expect(toItem(service, [0, 0], ctx({ zoom: 2 })).label.show).toBe(true);
        expect(toItem(service, [0, 0], ctx({ selected: 'svc:a' })).label.show).toBe(true);
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

    it('draws containment thin and without an arrow', () => {
        for (const rel of ['owns', 'implements', 'runs_on'] as const) {
            const link = toLink(edge(rel), ctx());
            expect(link.symbol, rel).toEqual(['none', 'none']);
            expect(link.lineStyle.width, rel).toBeLessThan(1);
        }
    });

    it('draws calls solid and declared dependencies dotted', () => {
        expect(toLink(edge('calls', { weight: 40 }), ctx()).lineStyle.type).toBe('solid');
        expect(toLink(edge('depends_on'), ctx()).lineStyle.type).toBe('dotted');
    });

    it('hides test and coverage edges until one end is selected', () => {
        expect(toLink(edge('tested_by'), ctx()).lineStyle.opacity).toBe(0);
        expect(toLink(edge('covers'), ctx()).lineStyle.opacity).toBe(0);
        const shown = toLink(edge('tested_by'), ctx({ selected: 'b' }));
        expect(shown.lineStyle.opacity).toBeGreaterThan(0);
    });

    it('draws what a run broke in the fail colour', () => {
        expect(toLink(edge('broke'), ctx()).lineStyle.color).toBe(t.fail);
    });
});

describe('tooltipHtml', () => {
    it('escapes names that come from telemetry', () => {
        const html = tooltipHtml(node({ label: '<script>alert(1)</script>' }), ctx());
        expect(html).not.toContain('<script>');
        expect(html).toContain('&lt;script&gt;');
    });

    it('says who suppressed a risk, until when and why', () => {
        const html = tooltipHtml(
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
        const html = tooltipHtml(
            node({
                risks: risks({ suppressed: 1, suppressed_by: 'ops', suppression_reason: 'known' }),
            }),
            ctx(),
        );
        expect(html).toContain('suppressed by ops with no expiry: known');
    });

    it('names the missing disciplines and the recommended kind on a gap', () => {
        const html = tooltipHtml(
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
        const html = tooltipHtml(
            node({ kind: 'service', conformance: 'target' }),
            ctx({ lens: 'conformance' }),
        );
        expect(html).toContain('conformance: target');
    });

    it('counts open and suppressed risks separately', () => {
        const html = tooltipHtml(
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
        const html = tooltipHtml(
            node({ kind: 'entity', flags: ['malformed_id', '<b>'], rollup }),
            ctx(),
        );
        expect(html).toContain('agreed form');
        expect(html).toContain('&lt;b&gt;');
        expect(html).toContain('5 of 6 systems untested');
    });
});
