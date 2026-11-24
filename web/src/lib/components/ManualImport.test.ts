import { render } from 'svelte/server';
import { describe, expect, it } from 'vitest';

import ManualImport from './ManualImport.svelte';

describe('the import of records', () => {
    it('takes a JSON file or pasted JSON, and a batch label', () => {
        const html = render(ManualImport, { props: { actor: 'alice', canWrite: true } }).body;
        expect(html).toContain('JSON file');
        expect(html).toContain('Records as JSON');
        expect(html).toContain('Batch label');
        expect(html).toContain('>Import as drafts<');
    });

    it('is not offered to a reader', () => {
        const html = render(ManualImport, { props: { actor: '', canWrite: false } }).body;
        expect(html).not.toContain('Import as drafts');
        expect(html).toContain('An operator can import records.');
    });
});
