import { render } from 'svelte/server';
import { describe, expect, it } from 'vitest';

import DrEmpty from './DrEmpty.svelte';
import { textOf } from './dr-test-data';
import { DR_GUIDE_URL } from '../dr';

describe('the DR page of an empty lake', () => {
    it('says there is no DR test yet and what to do next', () => {
        const html = render(DrEmpty, {}).body;
        expect(textOf(html)).toContain('No DR tests yet.');
        expect(html).toContain(`href="${DR_GUIDE_URL}"`);
        expect(html).toContain('href="/manual"');
        expect(html).toContain('href="/manual?tab=plans"');
    });
});
