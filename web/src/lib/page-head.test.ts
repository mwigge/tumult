import { readdirSync, readFileSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

// The head of every page is a PageHeader: no page writes the .page-head
// markup itself, and a link back to the page above has a class, not an
// inline style.

const SRC = fileURLToPath(new URL('..', import.meta.url));
const OWNER = join('lib', 'components', 'PageHeader.svelte');

/** The Svelte files under `dir`, as paths from src/. */
function svelteFiles(dir: string): string[] {
    return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
        const path = join(dir, entry.name);
        if (entry.isDirectory()) return svelteFiles(path);
        return entry.name.endsWith('.svelte') ? [relative(SRC, path)] : [];
    });
}

/** The page heads that `svelte` writes itself. */
const heads = (svelte: string) =>
    svelte.match(/class="[^"]*(?<![\w-])page-head(?![\w-])[^"]*"/g)?.length ?? 0;

/** The links back, `← name`, that `svelte` styles inline. */
const inlineBacks = (svelte: string) => svelte.match(/<a\b[^>]*\sstyle=[^>]*>\s*←/g)?.length ?? 0;

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

describe('the scan for page heads', () => {
    it('finds a page head and an inline link back in a page', () => {
        const page = [
            '<div class="page-head">',
            '    <a href="/runs" style="color: var(--text-dim)">← runs</a>',
            '    <h1>Run</h1>',
            '</div>',
            '<div class="page-head wide"><h1>Run</h1></div>',
            '<div class="panel-head"><a href="/runs" class="back">← runs</a></div>',
        ].join('\n');

        expect(heads(page)).toBe(2);
        expect(inlineBacks(page)).toBe(1);
    });
});

describe('the head of a page', () => {
    it('is written by PageHeader alone', () => {
        const pages = found(
            FILES.filter((file) => file !== OWNER),
            heads,
        );

        expect(pages, `${pages.total} page heads written by hand`).toEqual({ total: 0, where: [] });
        expect(FILES).toContain(OWNER);
    });

    it('has no link back with an inline style', () => {
        const links = found(FILES, inlineBacks);

        expect(links, `${links.total} links back with an inline style`).toEqual({
            total: 0,
            where: [],
        });
    });
});
