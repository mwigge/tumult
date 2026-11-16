import { readFileSync } from 'node:fs';

import { describe, expect, it } from 'vitest';

import { TAGLINE } from './brand';

describe('TAGLINE', () => {
    it('is the product line of the README and the site', () => {
        expect(TAGLINE).toBe('resilience intelligence');
    });

    it('is in the static page title', () => {
        const html = readFileSync('src/app.html', 'utf8');
        expect(html).toContain(`<title>Tumult — ${TAGLINE}</title>`);
    });

    it('is what the shell and the login page show', () => {
        for (const file of ['src/routes/+layout.svelte', 'src/routes/login/+page.svelte']) {
            const source = readFileSync(file, 'utf8');
            expect(source, file).toContain('{TAGLINE}');
            expect(source, file).not.toContain('analytics for your resilience work');
        }
    });
});
