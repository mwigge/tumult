import { describe, expect, it } from 'vitest';

import { chartTokens } from './echarts';
import {
    bandColor,
    bandGlyph,
    bandLabel,
    deltaClass,
    fmtDelta,
    fmtScore,
    sparkTrend,
    tileLabel,
    tileScore,
} from './org-score';

const DAY_MS = 86_400_000;
const NOW = Date.UTC(2026, 10, 18, 12);

describe('the score of a node', () => {
    it('shows a score with one decimal', () => {
        expect(fmtScore(87.5)).toBe('87.5');
        expect(fmtScore(0)).toBe('0.0');
    });

    it('shows a dash when the node has no score', () => {
        expect(fmtScore(null)).toBe('—');
    });
});

describe('the change of the score of a node', () => {
    it('shows a change with its sign and one decimal', () => {
        expect(fmtDelta(12.5)).toBe('+12.5');
        expect(fmtDelta(0)).toBe('+0.0');
        expect(fmtDelta(-3.25)).toBe('-3.3');
    });

    it('shows a dash when there is no change to show', () => {
        expect(fmtDelta(null)).toBe('—');
    });

    it('reads a rise as up, a fall as down and a small change as flat', () => {
        expect(deltaClass(12.5)).toBe('up');
        expect(deltaClass(-12.5)).toBe('down');
        expect(deltaClass(0.5)).toBe('flat');
        expect(deltaClass(-0.5)).toBe('flat');
    });

    it('reads no change to show as flat, in the neutral colour', () => {
        expect(deltaClass(null)).toBe('flat');
    });
});

describe('the band of a node', () => {
    it('draws each band of a score in its own hue with its glyph and its word', () => {
        expect([bandColor('good'), bandGlyph('good'), bandLabel('good')]).toEqual([
            '#009E73',
            '●',
            'good',
        ]);
        expect([bandColor('fair'), bandGlyph('fair'), bandLabel('fair')]).toEqual([
            '#c28600',
            '▲',
            'fair',
        ]);
        expect([bandColor('poor'), bandGlyph('poor'), bandLabel('poor')]).toEqual([
            '#D55E00',
            '×',
            'poor',
        ]);
    });

    it('draws the band of a node with no score as a dash in the neutral colour', () => {
        expect(bandLabel('unknown')).toBe('—');
        expect(bandGlyph('unknown')).toBe('');
        expect(bandColor('unknown')).toBe('#6f7c87');
        expect([bandColor('poor'), chartTokens('tumult-dark').fail]).not.toContain(
            bandColor('unknown'),
        );
    });
});

describe('the tile of a node in the treemap', () => {
    it('labels a tile with its name and its score', () => {
        expect(tileLabel('commerce', 62.5)).toBe('commerce\n62.5');
        expect(tileScore(62.5, 'fair')).toBe('score 62.5 (fair)');
    });

    it('labels a tile with no score as having none yet, also in its tooltip', () => {
        expect(tileLabel('commerce', null)).toBe('commerce\nno score yet');
        expect(tileScore(null, 'unknown')).toBe('no score yet');
    });
});

describe('the trend of the sparkline of a node', () => {
    it('places each of the ten instants of the range at the end of its step', () => {
        const all: [number, number][] = Array.from({ length: 10 }, (_, k) => [k + 1, 50 + k]);

        const trend = sparkTrend(all, 14 * DAY_MS, NOW);

        expect(trend?.ts[0]).toBe(NOW - 14 * DAY_MS + 1.4 * DAY_MS);
        expect(trend?.ts[9]).toBe(NOW);
        expect(trend?.observed).toEqual(all.map(([, v]) => v));
    });

    it('keeps the time of each instant when the first ones had nothing scored', () => {
        const trend = sparkTrend(
            [
                [8, 100],
                [9, 100],
                [10, 90],
            ],
            14 * DAY_MS,
            NOW,
        );

        expect(trend).toEqual({
            ts: [NOW - 2.8 * DAY_MS, NOW - 1.4 * DAY_MS, NOW],
            observed: [100, 100, 90],
        });
    });

    it('has no trend when no instant had a score', () => {
        expect(sparkTrend([], 14 * DAY_MS, NOW)).toBeNull();
    });
});
