import { readFileSync } from 'node:fs';

import { describe, expect, it } from 'vitest';

import {
    COMPLIANCE_SCORE,
    COMPLIANCE_SCORE_LINE,
    RESILIENCE_SCORE,
    RESILIENCE_SCORE_LINE,
} from './score-names';

/** Each page that shows a score, and the names and lines it shows. */
const PAGES: [string, string[]][] = [
    [
        'src/routes/resilience/+page.svelte',
        ['COMPLIANCE_SCORE', 'COMPLIANCE_SCORE_LINE', 'RESILIENCE_SCORE', 'RESILIENCE_SCORE_LINE'],
    ],
    ['src/routes/scores/+page.svelte', ['COMPLIANCE_SCORE', 'COMPLIANCE_SCORE_LINE']],
    ['src/lib/components/StarDrawer.svelte', ['COMPLIANCE_SCORE', 'COMPLIANCE_SCORE_LINE']],
];

describe('the names of the two scores', () => {
    it('are two names, each with a line of its own', () => {
        expect(COMPLIANCE_SCORE).not.toBe(RESILIENCE_SCORE);
        expect(COMPLIANCE_SCORE_LINE).not.toBe(RESILIENCE_SCORE_LINE);
    });

    it.each(PAGES)('are what %s shows, taken from this module', (file, shown) => {
        const source = readFileSync(file, 'utf8');
        for (const name of shown) expect(source, name).toContain(`{${name}}`);
        // A page writes no name and no line out on its own.
        for (const text of [
            COMPLIANCE_SCORE,
            COMPLIANCE_SCORE_LINE,
            RESILIENCE_SCORE,
            RESILIENCE_SCORE_LINE,
        ])
            expect(source, file).not.toContain(text);
    });
});
