import { readdirSync, readFileSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

// A badge is Badge.svelte, and its tone comes from the tone module: no page
// writes the class of a badge or works out its tone on its own.

const SRC = fileURLToPath(new URL('..', import.meta.url));

/** The pages, components and modules of src/, without tests and their data. */
function sources(dir: string): string[] {
    return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
        const path = join(dir, entry.name);
        if (entry.isDirectory()) return sources(path);
        if (!/\.(svelte|ts)$/.test(entry.name) || entry.name.includes('test')) return [];
        return [relative(SRC, path)];
    });
}

/** The only files that may write the class of a badge or choose its tone. */
const OWNERS = [join('lib', 'components', 'Badge.svelte'), join('lib', 'tone.ts')];

/** What a file must not hold, by the rule it breaks. */
const RULES: Record<string, RegExp> = {
    // class="badge ok", class="badge {tone}", class:badge, or a string 'badge ...'.
    'builds the class of a badge':
        /class(?::badge\b|=\s*(?:"[^"]*|'[^']*|\{[^}]*)\bbadge\b)|['"`]badge[\s'"`]/g,
    // <Badge tone={...}>: a tone that is worked out goes through a kind and a value.
    'works out the tone of a badge': /<Badge\b[^>]*?\btone=\{/g,
    // The copies the tone module replaces.
    'keeps its own tierClass, sevClass or statusClass': /\b(?:tierClass|sevClass|statusClass)\b/g,
};

/** How often `pattern` occurs in `file`. */
const count = (file: string, pattern: RegExp) =>
    readFileSync(join(SRC, file), 'utf8').match(pattern)?.length ?? 0;

describe('the tone of a badge', () => {
    it('is chosen only by Badge.svelte and the tone module', () => {
        const files = sources(SRC).filter((file) => !OWNERS.includes(file));
        expect(files).toContain(join('routes', '+layout.svelte'));
        const found = Object.entries(RULES).flatMap(([rule, pattern]) => {
            const hits = files
                .map((file) => ({ file, n: count(file, pattern) }))
                .filter(({ n }) => n > 0);
            if (hits.length === 0) return [];
            const total = hits.reduce((sum, { n }) => sum + n, 0);
            const where = hits.map(({ file, n }) => `${file} ${n}`).join(', ');
            return [`${rule}: ${total} in ${hits.length} files: ${where}`];
        });

        expect(found).toEqual([]);
    });
});
