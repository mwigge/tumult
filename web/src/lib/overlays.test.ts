import { describe, expect, it } from 'vitest';

import { chartTokens } from '#lib/echarts.js';
import { experimentOverlay, labelGap, outcomeColor, outcomeLegend, spacedLabels } from './overlays';
import type { ExperimentWindow } from './types';

const DARK = chartTokens('tumult-dark');
const LIGHT = chartTokens('tumult-light');

const HOUR_MS = 3_600_000;
const DAY = '2026-11-15';
const toDay = (): string => DAY;

function run(i: number, outcome: string | null = 'completed'): ExperimentWindow {
    const start = Date.UTC(2026, 10, 15, i) * 1e6;
    return {
        id: `run-${i}`,
        name: `exp-${i}`,
        start_ns: start,
        end_ns: start + HOUR_MS * 1e6,
        outcome,
    };
}

interface Marker {
    xAxis: number | string;
    name: string;
    runId: string | null;
    lineStyle: { color: string };
    label?: { show?: boolean; align?: string };
}
interface Overlay {
    markLine: { label: { position: string; color: string }; data: Marker[] };
    markArea: { data: { itemStyle?: { color: string } }[][] };
}

function overlayOf(
    runs: ExperimentWindow[],
    toX?: (ms: number) => number | string,
    gap?: number,
    axis?: { min: number; max: number },
    tokens = DARK,
): Overlay {
    const series = experimentOverlay(runs, tokens, toX, gap, axis);
    if (series === null) throw new Error('no overlay');
    return series as unknown as Overlay;
}

describe('experimentOverlay', () => {
    it('GIVEN no runs WHEN building THEN there is no overlay', () => {
        expect(experimentOverlay([], DARK)).toBeNull();
    });

    it('GIVEN runs at different times WHEN building THEN each run has its own named marker', () => {
        const markers = overlayOf([run(1), run(2, 'failed')]).markLine.data;
        expect(markers.map((m) => m.name)).toEqual(['exp-1 — completed', 'exp-2 — failed']);
        expect(markers.map((m) => m.runId)).toEqual(['run-1', 'run-2']);
        expect(markers[1]?.lineStyle.color).toBe(DARK.fail);
    });

    it('GIVEN many runs on one day column WHEN building THEN one marker counts them', () => {
        const runs = [run(1), run(2), run(3, 'deviated')];
        const markers = overlayOf(runs, toDay).markLine.data;
        expect(markers).toHaveLength(1);
        expect(markers[0]).toMatchObject({ xAxis: DAY, name: '3 runs', runId: null });
        expect(markers[0]?.lineStyle.color).toBe(DARK.text);
    });

    it('GIVEN runs on one column with one outcome WHEN building THEN the marker keeps its colour', () => {
        const markers = overlayOf([run(1, 'failed'), run(2, 'failed')], toDay).markLine.data;
        expect(markers[0]?.lineStyle.color).toBe(outcomeColor('failed', DARK));
    });

    it('GIVEN any overlay WHEN labelled THEN the label stays upright above the line', () => {
        // A label inside a vertical line turns with it: stacked runs drew
        // their names on top of each other, sideways, across the bar.
        expect(overlayOf([run(1)], toDay).markLine.label.position).toBe('end');
    });

    it('GIVEN runs on one column WHEN building THEN every run keeps its band', () => {
        expect(overlayOf([run(1), run(2)], toDay).markArea.data).toHaveLength(2);
    });

    it('GIVEN the light tokens WHEN building THEN bands, lines and labels are drawn in them', () => {
        const overlay = overlayOf([run(1), run(2, 'failed')], undefined, 0, undefined, LIGHT);
        expect(overlay.markArea.data.map(([from]) => from?.itemStyle?.color)).toEqual([
            `${LIGHT.ok}1a`,
            `${LIGHT.fail}1a`,
        ]);
        expect(overlay.markLine.data.map((m) => m.lineStyle.color)).toEqual([LIGHT.ok, LIGHT.fail]);
        expect(overlay.markLine.label.color).toBe(LIGHT.text);
    });
});

describe('spacedLabels', () => {
    it('GIVEN close x values WHEN spaced THEN only labels a full gap apart are shown', () => {
        expect(spacedLabels([0, 5, 50, 52, 200], 40)).toEqual([true, false, true, false, true]);
    });

    it('GIVEN unsorted x values WHEN spaced THEN each flag stays with its own value', () => {
        expect(spacedLabels([200, 52, 0, 50, 5], 40)).toEqual([true, false, true, true, false]);
    });

    it('GIVEN no gap WHEN spaced THEN every label is shown', () => {
        expect(spacedLabels([1, 1, 2], 0)).toEqual([true, true, true]);
    });
});

describe('labelGap', () => {
    it('GIVEN the x values of a chart WHEN measured THEN the gap is a share of the visible span', () => {
        const gap = labelGap([1000, 5000, 11_000]);
        expect(gap).toBeGreaterThan(0);
        expect(gap).toBeLessThan(10_000);
    });

    it('GIVEN no x values WHEN measured THEN there is no gap', () => {
        expect(labelGap([])).toBe(0);
    });
});

describe('experimentOverlay label spacing', () => {
    const MINUTE_NS = 60e9;
    const close: ExperimentWindow = {
        ...run(1),
        id: 'run-1b',
        start_ns: run(1).start_ns + MINUTE_NS,
    };

    it('GIVEN two runs a minute apart WHEN labels need an hour THEN the second label is hidden', () => {
        const markers = overlayOf([run(1), close], undefined, HOUR_MS).markLine.data;
        expect(markers).toHaveLength(2);
        expect(markers[0]?.label).toBeUndefined();
        expect(markers[1]?.label).toEqual({ show: false });
    });

    it('GIVEN a hidden label WHEN building THEN the line keeps its name and run', () => {
        const hidden = overlayOf([run(1), close], undefined, HOUR_MS).markLine.data[1];
        expect(hidden).toMatchObject({ name: 'exp-1 — completed', runId: 'run-1b' });
    });

    it('GIVEN no label gap WHEN building THEN every label is shown', () => {
        const markers = overlayOf([run(1), close]).markLine.data;
        expect(markers.every((m) => m.label === undefined)).toBe(true);
    });

    it('GIVEN a category axis WHEN a gap is given THEN labels are left alone', () => {
        const markers = overlayOf([run(1), run(2)], (ms) => `day-${ms}`, HOUR_MS).markLine.data;
        expect(markers.every((m) => m.label === undefined)).toBe(true);
    });
});

describe('experimentOverlay labels at the edge of the axis', () => {
    const at = (i: number): number => run(i).start_ns / 1e6;
    // Labels need two hours, so one hour is half a label.
    const GAP = 2 * HOUR_MS;
    const axis = { min: at(0), max: at(12) };
    const labelAt = (i: number) =>
        overlayOf([run(i)], undefined, GAP, axis).markLine.data[0]?.label;

    it('GIVEN a run closer than half a label to the end WHEN building THEN its name ends at the line', () => {
        expect(labelAt(12)).toEqual({ align: 'right' });
    });

    it('GIVEN a run closer than half a label to the start WHEN building THEN its name starts at the line', () => {
        expect(labelAt(0)).toEqual({ align: 'left' });
    });

    it('GIVEN a run half a label or more from both ends WHEN building THEN its name stays centred', () => {
        expect(labelAt(1)).toBeUndefined();
        expect(labelAt(6)).toBeUndefined();
        expect(labelAt(11)).toBeUndefined();
    });

    it('GIVEN a hidden label at the end WHEN building THEN it stays hidden', () => {
        const late: ExperimentWindow = {
            ...run(12),
            id: 'late',
            start_ns: run(12).start_ns - 60e9,
        };
        const markers = overlayOf([late, run(12)], undefined, GAP, axis).markLine.data;
        expect(markers.map((m) => m.label)).toEqual([{ align: 'right' }, { show: false }]);
    });

    it('GIVEN no axis WHEN building THEN names stay centred', () => {
        expect(overlayOf([run(12)], undefined, GAP).markLine.data[0]?.label).toBeUndefined();
    });
});

describe('outcomeLegend', () => {
    it('names each band colour with the statuses it stands for', () => {
        expect(outcomeLegend(DARK)).toEqual([
            { label: 'completed', color: DARK.ok },
            { label: 'deviated, failed, halted', color: DARK.fail },
            { label: 'aborted, interrupted, not ended', color: DARK.text },
        ]);
    });

    it('names the band colours in the tokens of the light theme', () => {
        expect(outcomeLegend(LIGHT).map((item) => item.color)).toEqual([
            LIGHT.ok,
            LIGHT.fail,
            LIGHT.text,
        ]);
    });

    it('gives each status the colour its band is drawn in', () => {
        for (const item of outcomeLegend(DARK)) {
            for (const status of item.label.split(', ')) {
                expect(outcomeColor(status === 'not ended' ? null : status, DARK)).toBe(item.color);
            }
        }
    });
});
