import { render } from 'svelte/server';
import { describe, expect, it } from 'vitest';

import { emptyAction, emptyParticipant, emptyStep } from '../manual';
import ManualActionsEditor from './ManualActionsEditor.svelte';
import ManualParticipantsEditor from './ManualParticipantsEditor.svelte';
import ManualStepsEditor from './ManualStepsEditor.svelte';

describe('the editors of the lists of a record', () => {
    it('label every field of a step by its number', () => {
        const html = render(ManualStepsEditor, {
            props: { rows: [emptyStep(), emptyStep()] },
        }).body;
        for (const field of ['title', 'actor', 'start', 'end', 'result', 'note']) {
            expect(html).toContain(`aria-label="Step 2 ${field}"`);
        }
        expect(html).toContain('aria-label="Remove step 2"');
        expect(html).toContain('>Add step<');
        expect(html).toContain('<option value="skipped"');
    });

    it('label the owner and the due date of an action', () => {
        const html = render(ManualActionsEditor, { props: { rows: [emptyAction()] } }).body;
        for (const field of ['title', 'owner', 'due date', 'ticket']) {
            expect(html).toContain(`aria-label="Action 1 ${field}"`);
        }
        expect(html).toContain('>Add action<');
    });

    it('label the name and the role of a participant', () => {
        const html = render(ManualParticipantsEditor, {
            props: { rows: [emptyParticipant()] },
        }).body;
        expect(html).toContain('aria-label="Participant 1 name"');
        expect(html).toContain('aria-label="Participant 1 role"');
        expect(html).toContain('>Add participant<');
    });

    it('say when a list is empty', () => {
        expect(render(ManualStepsEditor, { props: { rows: [] } }).body).toContain('No steps yet.');
    });
});
