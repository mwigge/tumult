import { readdirSync, readFileSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

// A table row that opens something on a click holds a link or a button,
// so that a keyboard can open it too. The click on the rest of the row
// stays, for a mouse.

const SRC = fileURLToPath(new URL('..', import.meta.url));

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

/** The rows with a click handler and no link or button in them. */
function closedRows(svelte: string): number {
    let count = 0;
    for (const match of svelte.matchAll(/<tr\b/g)) {
        const end = tagEnd(svelte, match.index);
        if (!/\sonclick=/.test(svelte.slice(match.index, end))) continue;
        const close = svelte.indexOf('</tr>', end);
        const body = svelte.slice(end, close === -1 ? undefined : close);
        if (!/<(?:a|button)\b/.test(body)) count++;
    }
    return count;
}

/** `path: n` for each file where `count` finds n > 0, and the sum of them. */
function found(files: string[], count: (svelte: string) => number) {
    const where = files.flatMap((file) => {
        const n = count(readFileSync(join(SRC, file), 'utf8'));
        return n > 0 ? [`${file}: ${n}`] : [];
    });
    const total = where.reduce((sum, line) => sum + Number(line.split(': ')[1]), 0);
    return { total, where };
}

describe('the scan for rows that open', () => {
    it('finds a row with a click handler and no link or button', () => {
        const page = [
            '<tr class="clickable" onclick={() => goto(`/runs/${row.id}`)}>',
            '    <td>{row.name}</td>',
            '</tr>',
            '<tr class="clickable" onclick={() => (open = row.id > 0)}>',
            '    <td><a class="row-link" href="/runs/{row.id}">{row.name}</a></td>',
            '</tr>',
            '<tr class="clickable" onclick={() => toggle(row.id)}>',
            '    <td><button class="row-toggle" aria-expanded={open}>{row.name}</button></td>',
            '</tr>',
            '<tr class="plain"><td>{row.name}</td></tr>',
        ].join('\n');

        expect(closedRows(page)).toBe(1);
    });
});

describe('a row that opens on a click', () => {
    it('holds a link or a button for the keyboard', () => {
        const rows = found(svelteFiles(SRC).sort(), closedRows);

        expect(rows, `${rows.total} rows that a keyboard cannot open`).toEqual({
            total: 0,
            where: [],
        });
    });
});
