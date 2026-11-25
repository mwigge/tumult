import { describe, expect, it } from 'vitest';

import {
    componentsLegend,
    componentsOption,
    coverageHue,
    heatmapLegend,
    heatmapOption,
    historyLegend,
    historyOption,
    historyStateText,
    proofDaysText,
    snapshotSkippedText,
    statusHue,
    STATUS_NAMES,
} from './dora';
import { chartTokens } from './echarts';
import type { DoraCell, DoraReport, MaturityHistoryPoint, MaturityReport } from './types';

const point = (over: Partial<MaturityHistoryPoint>): MaturityHistoryPoint => ({
    quarter: '2026-Q2',
    state: 'stored',
    source: 'snapshot',
    as_of: '2026-06-30',
    as_of_ns: 1,
    level: 2,
    index: 2.4,
    unknown_share: 0.5,
    reason: null,
    ...over,
});

describe('historyStateText', () => {
    it('names the day a quarter was stored on', () => {
        expect(historyStateText(point({}))).toBe('stored 2026-06-30');
        expect(historyStateText(point({ source: null }))).toBe('stored 2026-06-30');
    });

    it('says when the proof tool stored the day', () => {
        expect(historyStateText(point({ source: 'proof' }))).toBe(
            'stored 2026-06-30 by the proof tool',
        );
    });

    it('does not put a source on today or on a gap', () => {
        expect(historyStateText(point({ state: 'today', source: 'proof' }))).toBe('today');
        expect(historyStateText(point({ state: 'no_data', source: 'proof', as_of: null }))).toBe(
            'no data',
        );
    });
});

describe('snapshotSkippedText', () => {
    it('counts in the singular and the plural', () => {
        expect(snapshotSkippedText(1)).toBe('1 day skipped since the daemon started');
        expect(snapshotSkippedText(3)).toBe('3 days skipped since the daemon started');
    });

    it('says nothing before the first skipped day', () => {
        expect(snapshotSkippedText(0)).toBe('');
    });
});

describe('proofDaysText', () => {
    it('counts what counts when the setting is on', () => {
        expect(proofDaysText({ enabled: true, counted: 1, ignored: 0, note: 'n' })).toBe(
            '1 proof day counted',
        );
        expect(proofDaysText({ enabled: true, counted: 0, ignored: 0, note: 'n' })).toBe(
            '0 proof days counted',
        );
    });

    it('counts what is left out when the setting is off, and is silent with none', () => {
        expect(proofDaysText({ enabled: false, counted: 0, ignored: 4, note: null })).toBe(
            '4 days stored by the proof tool are not counted',
        );
        expect(proofDaysText({ enabled: false, counted: 0, ignored: 1, note: null })).toBe(
            '1 day stored by the proof tool is not counted',
        );
        expect(proofDaysText({ enabled: false, counted: 0, ignored: 0, note: null })).toBe('');
    });
});

/** The parts of a maturity report the component bars and the history read. */
const report = (): MaturityReport =>
    ({
        history: [
            point({ quarter: '2026-Q3' }),
            point({ quarter: '2026-Q4', state: 'today', level: 3, index: 3.1 }),
        ],
        weights: { conformance: 0.2, coverage: 0.3, slo: 0.25, recovery: 0.15, loop: 0.1 },
        components: { conformance: 81, coverage: 64.5, slo: null, recovery: 70, loop: 55 },
        component_states: {},
        cap: { rule: 'gate', level: 2, component: 'coverage', reason: 'coverage holds it' },
    }) as unknown as MaturityReport;

interface Line {
    name: string;
    lineStyle: { color: string; type?: string };
}
interface History {
    yAxis: { name?: string; min: number; max: number };
    series: Line[];
}
interface Bars {
    xAxis: { name?: string; min: number; max: number };
    series: { data: { component: string; itemStyle: { color: string } }[] }[];
}

describe('historyOption', () => {
    it('names the axis of the level with its scale and each of the two lines', () => {
        const option = historyOption(report(), 'tumult-dark') as unknown as History;
        expect(option.yAxis).toMatchObject({ name: 'level (1 to 5)', min: 1, max: 5 });
        expect(option.series.map((s) => s.name)).toEqual(['Level', 'Index']);
    });
});

describe('historyLegend', () => {
    it('shows each line in the colour and the stroke the chart draws it in', () => {
        for (const theme of ['tumult-dark', 'tumult-light'] as const) {
            const option = historyOption(report(), theme) as unknown as History;
            expect(historyLegend(theme)).toEqual(
                option.series.map((s) => ({
                    label: s.name,
                    color: s.lineStyle.color,
                    mark: s.lineStyle.type === 'dashed' ? 'dashed' : 'line',
                })),
            );
        }
        expect(historyLegend('tumult-dark').map((item) => item.label)).toEqual(['Level', 'Index']);
    });
});

describe('componentsOption', () => {
    it('names the score axis with its scale', () => {
        const option = componentsOption(report(), 'tumult-dark') as unknown as Bars;
        expect(option.xAxis).toMatchObject({ name: 'score (0 to 100)', min: 0, max: 100 });
    });
});

describe('componentsLegend', () => {
    it('says that the warn colour is the component that caps the level', () => {
        const t = chartTokens('tumult-dark');
        expect(componentsLegend('tumult-dark')).toEqual([
            { label: 'component score', color: t.accent },
            { label: 'the component that caps the level', color: t.warn },
        ]);
    });

    it('takes its colours from the bars', () => {
        const option = componentsOption(report(), 'tumult-light') as unknown as Bars;
        const [score, capped] = componentsLegend('tumult-light');
        const color = (key: string) =>
            option.series[0].data.find((d) => d.component === key)?.itemStyle.color;
        expect(color('coverage')).toBe(capped.color);
        expect(color('conformance')).toBe(score.color);
    });
});

describe('heatmapLegend', () => {
    const swatches = (mode: 'coverage' | 'status') =>
        heatmapLegend(mode)
            .filter((item) => item.color)
            .map((item) => [item.label, item.color]);

    it('names the coverage hues with the two stops', () => {
        expect(swatches('coverage')).toEqual([
            ['below 50 %', coverageHue(0.49)],
            ['from 50 %', coverageHue(0.5)],
            ['from 80 %', coverageHue(0.8)],
        ]);
    });

    it('names the hue of each evidence status with its rank', () => {
        expect(swatches('status')).toEqual(
            STATUS_NAMES.map((name, rank) => [`${rank} ${name}`, statusHue(rank)]),
        );
    });

    it('names the hatch and the two signs in both views', () => {
        for (const mode of ['coverage', 'status'] as const) {
            expect(heatmapLegend(mode).slice(-3)).toEqual([
                { label: 'hatched = unknown share', mark: 'hatch' },
                { label: 'no source connected', glyph: '?' },
                { label: 'no events in window', glyph: '–' },
            ]);
        }
    });
});

describe('heatmapOption', () => {
    const cell = (article: string): DoraCell => ({
        article,
        entity_id: 'cif-0001',
        coverage: 0.5,
        unknown: 0,
        state: 'known',
        status: 'existing',
        status_rank: 2,
        gaps: 1,
    });
    const report = {
        articles: [{ article: '11' }, { article: 'rts1774' }],
        rows: [{ entity_id: 'cif-0001', label: 'Payments', tier: 'critical' }],
        cells: [cell('11'), cell('rts1774')],
    } as unknown as DoraReport;

    interface Heatmap {
        xAxis: { data: string[] };
        tooltip: { formatter: (p: { data: unknown }) => string };
        series: { data: (DoraCell & { row: string })[] }[];
    }

    it('names a DORA article by its number and the RTS rows by the act', () => {
        const option = heatmapOption(report, 'coverage', 'tumult-dark') as unknown as Heatmap;
        expect.soft(option.xAxis.data).toEqual(['Art. 11', 'RTS 2024/1774']);
        const tips = option.series[0].data.map((data) => option.tooltip.formatter({ data }));
        expect(tips[0]).toMatch(/^<b>Art\. 11<\/b> · Payments/);
        expect(tips[1]).toMatch(/^<b>RTS 2024\/1774<\/b> · Payments/);
    });
});
