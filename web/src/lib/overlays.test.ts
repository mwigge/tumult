import { describe, expect, it } from 'vitest';

import { CHART } from '#lib/echarts.js';
import { experimentOverlay, outcomeColor } from './overlays';
import type { ExperimentWindow } from './types';

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
}
interface Overlay {
    markLine: { label: { position: string }; data: Marker[] };
    markArea: { data: unknown[] };
}

function overlayOf(runs: ExperimentWindow[], toX?: (ms: number) => number | string): Overlay {
    const series = experimentOverlay(runs, toX);
    if (series === null) throw new Error('no overlay');
    return series as unknown as Overlay;
}

describe('experimentOverlay', () => {
    it('GIVEN no runs WHEN building THEN there is no overlay', () => {
        expect(experimentOverlay([])).toBeNull();
    });

    it('GIVEN runs at different times WHEN building THEN each run has its own named marker', () => {
        const markers = overlayOf([run(1), run(2, 'failed')]).markLine.data;
        expect(markers.map((m) => m.name)).toEqual(['exp-1 — completed', 'exp-2 — failed']);
        expect(markers.map((m) => m.runId)).toEqual(['run-1', 'run-2']);
        expect(markers[1]?.lineStyle.color).toBe(CHART.fail);
    });

    it('GIVEN many runs on one day column WHEN building THEN one marker counts them', () => {
        const runs = [run(1), run(2), run(3, 'deviated')];
        const markers = overlayOf(runs, toDay).markLine.data;
        expect(markers).toHaveLength(1);
        expect(markers[0]).toMatchObject({ xAxis: DAY, name: '3 runs', runId: null });
        expect(markers[0]?.lineStyle.color).toBe(CHART.text);
    });

    it('GIVEN runs on one column with one outcome WHEN building THEN the marker keeps its colour', () => {
        const markers = overlayOf([run(1, 'failed'), run(2, 'failed')], toDay).markLine.data;
        expect(markers[0]?.lineStyle.color).toBe(outcomeColor('failed'));
    });

    it('GIVEN any overlay WHEN labelled THEN the label stays upright above the line', () => {
        // A label inside a vertical line turns with it: stacked runs drew
        // their names on top of each other, sideways, across the bar.
        expect(overlayOf([run(1)], toDay).markLine.label.position).toBe('end');
    });

    it('GIVEN runs on one column WHEN building THEN every run keeps its band', () => {
        expect(overlayOf([run(1), run(2)], toDay).markArea.data).toHaveLength(2);
    });
});
