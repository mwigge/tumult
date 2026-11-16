import { describe, expect, it } from 'vitest';

import type { StarMap } from '../types';
import { missingNote, missingSources } from './sources';

type Sources = StarMap['sources'];

const RESERVED_OFF: Sources = {
    slo: 'unavailable',
    dora: 'unavailable',
    maturity: 'unavailable',
    conformance: 'ok',
    graph: 'ok',
};

describe('missingSources', () => {
    it('GIVEN only the reserved overlays are off WHEN read THEN nothing is missing', () => {
        expect(missingSources(RESERVED_OFF)).toEqual([]);
    });

    it('GIVEN no map yet WHEN read THEN nothing is missing', () => {
        expect(missingSources(undefined)).toEqual([]);
    });

    it.each([
        ['graph', ['the graph']],
        ['conformance', ['OTel conformance']],
    ] as const)(
        'GIVEN the drawn source %s failed WHEN read THEN it is named in plain words',
        (source, names) => {
            expect(missingSources({ ...RESERVED_OFF, [source]: 'unavailable' })).toEqual(names);
        },
    );

    it('GIVEN both drawn sources failed WHEN read THEN both are named in a fixed order', () => {
        const sources: Sources = {
            ...RESERVED_OFF,
            graph: 'unavailable',
            conformance: 'unavailable',
        };
        expect(missingSources(sources)).toEqual(['the graph', 'OTel conformance']);
    });
});

describe('missingNote', () => {
    it.each([
        [[], null],
        [['OTel conformance'], 'Could not load OTel conformance; the map is drawn without it.'],
        [
            ['the graph', 'OTel conformance'],
            'Could not load the graph and OTel conformance; the map is drawn without them.',
        ],
    ] as const)('GIVEN missing %j WHEN noted THEN the note is %j', (names, note) => {
        expect(missingNote([...names])).toBe(note);
    });
});
