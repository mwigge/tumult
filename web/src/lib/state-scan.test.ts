import { readdirSync, readFileSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

// Every empty, error and loading state is a State: no page writes the
// .state or .skeleton markup itself, and the height of a loading block is
// the `height` of its State, not an inline style.

const SRC = fileURLToPath(new URL('..', import.meta.url));
const OWNER = join('lib', 'components', 'State.svelte');

/** The Svelte files under `dir`, as paths from src/. */
function svelteFiles(dir: string): string[] {
    return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
        const path = join(dir, entry.name);
        if (entry.isDirectory()) return svelteFiles(path);
        return entry.name.endsWith('.svelte') ? [relative(SRC, path)] : [];
    });
}

/** The states and skeletons that `svelte` writes itself, as a class. */
const states = (svelte: string) =>
    svelte.match(/class="[^"]*(?<![\w-])(?:state|skeleton)(?![\w-])[^"]*"/g)?.length ?? 0;

/** The loading blocks, States or skeletons, whose height is an inline style. */
const inlineHeights = (svelte: string) =>
    [
        /<State\b[^>]*\sstyle="[^"]*(?<![\w-])height\s*:/g,
        /<[a-z]+\b[^>]*\sclass="[^"]*(?<![\w-])skeleton(?![\w-])[^"]*"[^>]*\sstyle="[^"]*(?<![\w-])height\s*:/g,
    ].reduce((sum, pattern) => sum + (svelte.match(pattern)?.length ?? 0), 0);

/** `path: n` for each file where `count` finds n > 0, and the sum of them. */
function found(files: string[], count: (svelte: string) => number) {
    const where = files.flatMap((file) => {
        const n = count(readFileSync(join(SRC, file), 'utf8'));
        return n > 0 ? [`${file}: ${n}`] : [];
    });
    const total = where.reduce((sum, line) => sum + Number(line.split(': ')[1]), 0);
    return { total, where };
}

const FILES = svelteFiles(SRC).sort();

describe('the scan for states', () => {
    it('finds a state, an error, a skeleton and an inline height in a page', () => {
        const page = [
            '<div class="state">No runs.</div>',
            '<div class="state error panel">{error}</div>',
            '<div class="panel state block">no journal</div>',
            '<div class="skeleton" style="height: 120px"></div>',
            '<State kind="loading" style="height: 40px" />',
            '<State kind="loading" height={40} style="margin-top: var(--space-3)" />',
            '<div class="state-line">not a state</div>',
            '<div class="statement">not a state</div>',
        ].join('\n');

        expect(states(page)).toBe(4);
        expect(inlineHeights(page)).toBe(2);
    });
});

describe('a state of a page', () => {
    it('is written by State alone', () => {
        const pages = found(
            FILES.filter((file) => file !== OWNER),
            states,
        );

        expect(pages, `${pages.total} states written by hand`).toEqual({ total: 0, where: [] });
        expect(FILES).toContain(OWNER);
    });

    it('has no loading block with an inline height', () => {
        const heights = found(FILES, inlineHeights);

        expect(heights, `${heights.total} loading blocks with an inline height`).toEqual({
            total: 0,
            where: [],
        });
    });
});
