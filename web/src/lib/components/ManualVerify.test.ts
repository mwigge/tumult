import { render } from 'svelte/server';
import { describe, expect, it } from 'vitest';

import type { ManualVerdict } from '../types';
import ManualVerify from './ManualVerify.svelte';

const run = async (): Promise<ManualVerdict> => {
    throw new Error('not run in a server render');
};

const shown = (verdict: ManualVerdict | null) =>
    render(ManualVerify, { props: { run, verdict } }).body;

describe('the audit verify on the page', () => {
    it('is a button until it runs', () => {
        const html = shown(null);
        expect(html).toContain('>Verify audit trail<');
        expect(html).not.toContain('Audit trail ok');
    });

    it('says ok when the verify finds no problem', () => {
        const html = shown({
            experiment_id: 'rec-1',
            ok: true,
            chain_valid: true,
            hash_version: 2,
            legacy_rows: 0,
            problems: [],
        });
        expect(html).toContain('Audit trail ok: no problem found.');
        expect(html).toContain('seal form 2');
    });

    it('lists each problem with its table, its row and its field', () => {
        const html = shown({
            experiment_id: 'rec-1',
            ok: false,
            chain_valid: false,
            hash_version: 2,
            legacy_rows: 1,
            problems: [
                {
                    table: 'evidence_attachments',
                    row: 'att-1',
                    field: 'file_hash',
                    problem: 'the stored file does not hash to file_hash',
                },
                { table: 'manual_signoffs', row: 'so-1', field: null, problem: 'link broken' },
            ],
        });
        expect(html).toContain('Audit trail not ok: 2 problems.');
        expect(html).toContain('evidence_attachments');
        expect(html).toContain('att-1');
        expect(html).toContain('file_hash');
        expect(html).toContain('the stored file does not hash to file_hash');
        expect(html).toContain('link broken');
        expect(html).toContain('1 legacy row');
    });
});
