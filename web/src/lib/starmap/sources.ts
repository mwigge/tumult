// Which sources of the Star Map failed to load, in words for the page.
//
// `slo`, `dora` and `maturity` are reserved for overlays the map does not
// draw yet, so the API always reports them `unavailable`: that is not news
// for a reader. Only a source the map draws today is worth a line.

import type { StarMap } from '../types.js';

type Sources = StarMap['sources'];

/** The sources the map draws, in the order they are named. */
const DRAWN: readonly (readonly [keyof Sources, string])[] = [
    ['graph', 'the graph'],
    ['conformance', 'OTel conformance'],
];

/** Plain names of the drawn sources that did not answer. */
export function missingSources(sources: Sources | undefined): string[] {
    if (!sources) return [];
    return DRAWN.filter(([key]) => sources[key] === 'unavailable').map(([, name]) => name);
}

/** The line under the map, or null when every drawn source answered. */
export function missingNote(names: string[]): string | null {
    if (names.length === 0) return null;
    const them = names.length === 1 ? 'it' : 'them';
    return `Could not load ${names.join(' and ')}; the map is drawn without ${them}.`;
}
