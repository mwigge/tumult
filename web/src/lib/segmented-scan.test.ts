import { readdirSync, readFileSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

// A group of buttons where one is chosen is Segmented.svelte, and tabs are
// Tabs.svelte: no page draws the group itself, and every button of a group
// says whether it is pressed.

const SRC = fileURLToPath(new URL('..', import.meta.url));

/** The pages and components of src/. */
function sources(dir: string): string[] {
    return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
        const path = join(dir, entry.name);
        if (entry.isDirectory()) return sources(path);
        return entry.name.endsWith('.svelte') ? [relative(SRC, path)] : [];
    });
}

/** The classes of theme.css that draw a group of buttons as one control. */
const LOOKS = /\bclass="(?:[^"]*\s)?(?:seg|views|toggle)(?:\s[^"]*)?"/;

/** One HTML tag of the markup: its name, its text and where it ends. */
interface Tag {
    name: string;
    text: string;
    end: number;
}

/** The tag that opens at `start`, read up to the `>` outside braces and quotes. */
function tagAt(source: string, start: number): Tag {
    let depth = 0;
    let quote = '';
    for (let i = start + 1; i < source.length; i++) {
        const c = source[i];
        if (quote) {
            if (c === quote) quote = '';
        } else if (c === '{') depth++;
        else if (c === '}') depth--;
        else if (depth === 0 && (c === '"' || c === "'")) quote = c;
        else if (depth === 0 && c === '>') {
            const name = /^<([a-z][\w-]*)/.exec(source.slice(start))?.[1] ?? '';
            return { name, text: source.slice(start, i + 1), end: i + 1 };
        }
    }
    return { name: '', text: source.slice(start), end: source.length };
}

/** The markup inside the element that `open` opens, up to its own closing tag. */
function inner(source: string, open: Tag): string {
    const tags = new RegExp(`<(/?)${open.name}\\b`, 'g');
    tags.lastIndex = open.end;
    let depth = 1;
    for (let m = tags.exec(source); m; m = tags.exec(source)) {
        depth += m[1] ? -1 : 1;
        if (depth === 0) return source.slice(open.end, m.index);
    }
    return source.slice(open.end);
}

/** Every element of `source` that groups buttons: a role of group, or a look of theme.css. */
function groups(source: string): { open: Tag; body: string }[] {
    const found: { open: Tag; body: string }[] = [];
    for (const m of source.matchAll(/<[a-z][\w-]*\b/g)) {
        const open = tagAt(source, m.index);
        if (!/\brole="group"/.test(open.text) && !LOOKS.test(open.text)) continue;
        found.push({ open, body: inner(source, open) });
    }
    return found;
}

/** The buttons of `body` that do not say whether they are pressed. */
const unpressed = (body: string) =>
    [...body.matchAll(/<button\b/g)]
        .map((m) => tagAt(body, m.index).text)
        .filter((tag) => !/\baria-pressed=/.test(tag)).length;

/** What a file must not hold, by the rule it breaks: the count in each file. */
const RULES: Record<string, (source: string) => number> = {
    'draws a segmented group itself': (source) =>
        groups(source).filter(({ open }) => LOOKS.test(open.text)).length,
    'has a button group without aria-pressed': (source) =>
        groups(source).filter(({ body }) => unpressed(body) > 0).length,
};

describe('a group of buttons', () => {
    it('is a Segmented control or tabs, and says which button is pressed', () => {
        const files = sources(SRC);
        expect(files).toContain(join('routes', '+layout.svelte'));
        const found = Object.entries(RULES).flatMap(([rule, countIn]) => {
            const hits = files
                .map((file) => ({ file, n: countIn(readFileSync(join(SRC, file), 'utf8')) }))
                .filter(({ n }) => n > 0);
            if (hits.length === 0) return [];
            const total = hits.reduce((sum, { n }) => sum + n, 0);
            const where = hits.map(({ file, n }) => `${file} ${n}`).join(', ');
            return [`${rule}: ${total} in ${hits.length} files: ${where}`];
        });

        expect(found).toEqual([]);
    });

    it('is found by the scan, with buttons that do not say they are pressed', () => {
        const page = [
            '<div class="seg" role="group" aria-label="x">',
            '    <button class:active={a} onclick={() => (a = true)}>one</button>',
            '    <div><button aria-pressed={b}>two</button></div>',
            '</div>',
            '<div class="kinds" role="group"><button aria-pressed={c}>three</button></div>',
            '<div class="toggle"><button onclick={() => go()} aria-pressed={d}>four</button></div>',
        ].join('\n');
        expect(RULES['draws a segmented group itself'](page)).toBe(2);
        expect(RULES['has a button group without aria-pressed'](page)).toBe(1);
        expect(RULES['draws a segmented group itself']('<Segmented class="toggle" />')).toBe(0);
    });
});
