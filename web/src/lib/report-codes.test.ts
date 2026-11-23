import { describe, expect, it } from 'vitest';

import { templateCode } from './report-codes';

describe('templateCode', () => {
    it('GIVEN each template the daemon makes WHEN coded THEN it is R1 to R5', () => {
        expect(templateCode('executive-digest')).toBe('R1');
        expect(templateCode('evidence-pack')).toBe('R2');
        expect(templateCode('game-day')).toBe('R3');
        expect(templateCode('dr-test')).toBe('R4');
        expect(templateCode('dr-exercise')).toBe('R5');
    });

    it('GIVEN a template the page does not know WHEN coded THEN it shows its own name', () => {
        expect(templateCode('board-pack')).toBe('board-pack');
        expect(templateCode('constructor')).toBe('constructor');
    });
});
