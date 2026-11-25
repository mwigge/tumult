// What the org score view draws for a node of `/api/scores/tree`: its
// score, its change, its band, its treemap tile and its trend. A node with
// no scored leaf has no score (`null`) and the band `unknown`; each place
// then shows a dash in the neutral colour, never 0 or the red of a failure.

import { BAND, UNSCORED } from './chart-theme';

/** What a missing value shows. */
const DASH = '—';
/** What a treemap tile with no score says. */
const NO_SCORE = 'no score yet';

/** Band hues from the Okabe-Ito palette (colour-blind safe). */
const BAND_COLOR: Readonly<Record<string, string>> = BAND;
const BAND_GLYPH: Readonly<Record<string, string>> = { good: '●', fair: '▲', poor: '×' };
/** The colour of a band with no score. */
const NEUTRAL = UNSCORED;

/** The colour of a band; neutral for `unknown`, the band of no score. */
export const bandColor = (band: string) => BAND_COLOR[band] ?? NEUTRAL;

/** The glyph of a band; none for `unknown`. */
export const bandGlyph = (band: string) => BAND_GLYPH[band] ?? '';

/** The word of a band; a dash for `unknown`. */
export const bandLabel = (band: string) => (Object.hasOwn(BAND_COLOR, band) ? band : DASH);

/** A score with one decimal; a dash for no score. */
export const fmtScore = (score: number | null) => (score === null ? DASH : score.toFixed(1));

/** A change with its sign and one decimal; a dash for no change to show. */
export const fmtDelta = (delta: number | null) =>
    delta === null ? DASH : `${delta >= 0 ? '+' : ''}${delta.toFixed(1)}`;

/** The class of a change: up, down, or flat (neutral) for a small change or none. */
export function deltaClass(delta: number | null): 'up' | 'down' | 'flat' {
    if (delta === null) return 'flat';
    if (delta > 0.5) return 'up';
    return delta < -0.5 ? 'down' : 'flat';
}

/** The label of a treemap tile: its name over its score, or over that it has none yet. */
export const tileLabel = (name: string, score: number | null) =>
    `${name}\n${score === null ? NO_SCORE : score.toFixed(1)}`;

/** The score line of the tooltip of a treemap tile. */
export const tileScore = (score: number | null, band: string) =>
    score === null ? NO_SCORE : `score ${score.toFixed(1)} (${band})`;

/**
 * The instants the API samples for the sparkline of a node: point `i` is
 * the end of the `i`th of ten equal steps of the range (`POINTS` in
 * `scores_tree`, tumult-api/src/handlers/reports.rs).
 */
const SPARK_INSTANTS = 10;

/**
 * The trend of the sparkline of a node over `rangeMs` up to `nowMs`. An
 * instant with nothing scored has no point, so a point is placed by its
 * instant, not by how many points came before it. `null` with no point.
 */
export function sparkTrend(
    sparkline: [number, number][],
    rangeMs: number,
    nowMs: number,
): { ts: number[]; observed: number[] } | null {
    if (sparkline.length === 0) return null;
    return {
        ts: sparkline.map(([i]) => nowMs - rangeMs + (rangeMs * i) / SPARK_INSTANTS),
        observed: sparkline.map(([, score]) => score),
    };
}
