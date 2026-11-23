import { render } from 'svelte/server';
import { describe, expect, it } from 'vitest';

import type { EvidenceAttachment } from '../types';
import ManualAttachments from './ManualAttachments.svelte';

const HASH = '3f9a1c0d5e7b2a4c6e8f0a1b3c5d7e9f1a3b5c7d9e1f3a5b7c9d1e3f5a7b9c1d';

const file: EvidenceAttachment = {
    id: 'att-1',
    experiment_id: 'rec-1',
    kind: 'file',
    uri: `attachments/3f/${HASH}`,
    label: 'runbook.pdf',
    file_hash: HASH,
    media_type: 'application/pdf',
    size_bytes: 1536,
    added_by: 'alice',
    added_at_ns: Date.UTC(2026, 10, 20) * 1e6,
};

const link: EvidenceAttachment = {
    ...file,
    id: 'att-2',
    kind: 'ticket',
    uri: 'https://tickets.example/OPS-7',
    label: 'OPS-7',
    file_hash: null,
    media_type: null,
    size_bytes: null,
};

const shown = (
    props: Partial<{ attachments: EvidenceAttachment[]; status: string; canWrite: boolean }>,
) =>
    render(ManualAttachments, {
        props: {
            experimentId: 'rec-1',
            attachments: [file, link],
            status: 'draft',
            actor: 'alice',
            canWrite: true,
            ...props,
        },
    }).body;

describe('the evidence of a record', () => {
    it('lists a file with its type, its size and its hash, and a download', () => {
        const html = shown({});
        expect(html).toContain('runbook.pdf');
        expect(html).toContain('application/pdf');
        expect(html).toContain('1.5 KB');
        expect(html).toContain(`title="SHA-256 ${HASH}"`);
        expect(html).toContain(HASH.slice(0, 12));
        expect(html).toContain('aria-label="Download runbook.pdf"');
    });

    it('lists a link as a link that opens apart from the page', () => {
        const html = shown({});
        expect(html).toContain('href="https://tickets.example/OPS-7"');
        expect(html).toContain('rel="noreferrer"');
    });

    it('takes a file in any status, and a link only before the sign-off', () => {
        const draft = shown({});
        expect(draft).toContain('File to attach');
        expect(draft).toContain('Upload file');
        expect(draft).toContain('Add link');
        const verified = shown({ status: 'verified' });
        expect(verified).toContain('Upload file');
        expect(verified).not.toContain('Add link');
    });

    it('offers no upload to a reader', () => {
        const html = shown({ canWrite: false });
        expect(html).not.toContain('Upload file');
        expect(html).not.toContain('Add link');
        expect(html).toContain('aria-label="Download runbook.pdf"');
    });

    it('says when nothing is attached', () => {
        expect(shown({ attachments: [] })).toContain('No evidence attached yet.');
    });
});
