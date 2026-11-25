import { readdirSync, readFileSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

// Every field of a page has a name that a screen reader says: it sits in
// a <label>, a <label for> names it, or it has an aria-label. Every page
// names itself in the title of the browser tab, through PageHeader or
// Title. The browser test names.spec.ts reads the names of the fields of
// the mocked pages; this scan also reaches the fields that only a click
// or a role shows.

const SRC = fileURLToPath(new URL('..', import.meta.url));
const ROUTES = join(SRC, 'routes');

/** The Svelte files under `dir`, as paths from src/. */
function svelteFiles(dir: string): string[] {
    return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
        const path = join(dir, entry.name);
        if (entry.isDirectory()) return svelteFiles(path);
        return entry.name.endsWith('.svelte') ? [relative(SRC, path)] : [];
    });
}

/** The end of the tag that starts at `at`: the first `>` outside quotes and braces. */
function tagEnd(svelte: string, at: number): number {
    let depth = 0;
    let quote = '';
    for (let i = at; i < svelte.length; i++) {
        const c = svelte[i];
        if (quote) {
            if (c === quote) quote = '';
        } else if (c === '"' || c === "'" || c === '`') quote = c;
        else if (c === '{') depth++;
        else if (c === '}') depth--;
        else if (c === '>' && depth === 0) return i;
    }
    return svelte.length;
}

/** The markup of a component: its script, style and comments left out. */
const markupOf = (svelte: string) =>
    svelte
        .replace(/<script\b[\s\S]*?<\/script>/g, '')
        .replace(/<style\b[\s\S]*?<\/style>/g, '')
        .replace(/<!--[\s\S]*?-->/g, '');

/** The value of the attribute `name` of `tag`, or null when it has none. */
function attribute(tag: string, name: string): string | null {
    const match = new RegExp(`\\s${name}=("[^"]*"|\\{[^}]*\\})`).exec(tag);
    return match ? match[1] : null;
}

/** Input types that take their name from their value or need none. */
const UNNAMED_TYPES = ['"hidden"', '"submit"', '"button"', '"reset"'];

/** The fields that nothing names: no <label> around them, no <label for>, no aria-label. */
function unnamedFields(svelte: string): number {
    const markup = markupOf(svelte);
    const tags = [...markup.matchAll(/<(\/?)(label|input|select|textarea)\b/g)].map((m) => ({
        close: m[1] === '/',
        name: m[2],
        text: markup.slice(m.index, tagEnd(markup, m.index) + 1),
    }));
    const labelled = tags
        .filter((tag) => tag.name === 'label' && !tag.close)
        .map((tag) => attribute(tag.text, 'for'))
        .filter((value) => value !== null);
    let depth = 0;
    let count = 0;
    for (const tag of tags) {
        if (tag.name === 'label') {
            depth += tag.close ? -1 : 1;
            continue;
        }
        if (tag.close || depth > 0) continue;
        if (UNNAMED_TYPES.includes(attribute(tag.text, 'type') ?? '')) continue;
        if (/\saria-label(?:ledby)?=/.test(tag.text)) continue;
        const id = attribute(tag.text, 'id');
        if (id !== null && labelled.includes(id)) continue;
        count++;
    }
    return count;
}

/** 1 for a page that names itself through neither PageHeader nor Title. */
const untitled = (svelte: string) => (/<(?:PageHeader|Title)\b/.test(svelte) ? 0 : 1);

/** `path: n` for each file where `count` finds n > 0, and the sum of them. */
function found(files: string[], count: (svelte: string) => number) {
    const where = files.flatMap((file) => {
        const n = count(readFileSync(join(SRC, file), 'utf8'));
        return n > 0 ? [`${file}: ${n}`] : [];
    });
    const total = where.reduce((sum, line) => sum + Number(line.split(': ')[1]), 0);
    return { total, where };
}

describe('the scan for names', () => {
    it('finds a field that nothing names', () => {
        const page = [
            '<script>let q = $state("");</script>',
            '<input type="search" placeholder="search…" bind:value={q} />',
            '<select onchange={(e) => (q = e.currentTarget.value > "" ? "a" : "b")}>',
            '    <option value="">all</option>',
            '</select>',
            '<label>Name <input type="text" bind:value={q} /></label>',
            '<label for="due">Due</label><input id="due" type="date" />',
            '<input type="text" aria-label="Note" />',
            '<textarea aria-labelledby="note-head"></textarea>',
            '<input type="hidden" name="token" />',
            '<!-- <input type="text" /> -->',
        ].join('\n');

        expect(unnamedFields(page)).toBe(2);
    });

    it('finds a page with no title', () => {
        expect(untitled('<PageHeader title="Runs" />')).toBe(0);
        expect(untitled('<Title name="Sign in" />')).toBe(0);
        expect(untitled('<div class="login-wrap"></div>')).toBe(1);
    });
});

describe('a field', () => {
    it('has a name', () => {
        const fields = found(svelteFiles(SRC).sort(), unnamedFields);

        expect(fields, `${fields.total} fields with no name`).toEqual({ total: 0, where: [] });
    });
});

describe('a page', () => {
    it('names itself in the title of the tab', () => {
        const pages = svelteFiles(ROUTES).filter((file) => file.endsWith('+page.svelte'));
        const titles = found(pages.sort(), untitled);

        expect(titles, `${titles.total} pages with no title`).toEqual({ total: 0, where: [] });
        expect(pages.length).toBeGreaterThan(25);
    });
});
