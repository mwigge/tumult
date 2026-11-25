import { readdirSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

// Charts draw on a canvas and take their colours, fonts and theme from
// the tokens, through chart-theme.ts and echarts.ts. These scans keep
// chart code from writing its own.

/**
 * The script code under src/: each TS file and the script blocks of each
 * Svelte file, with no test and not the generated tokens.
 */
function scripts(): { file: string; code: string }[] {
    const src = fileURLToPath(new URL('..', import.meta.url));
    return readdirSync(src, { recursive: true, encoding: 'utf8' })
        .filter((file) => /\.(ts|svelte)$/.test(file) && !/\.test\.ts$|tokens\.gen\.ts$/.test(file))
        .sort()
        .map((file) => {
            const text = readFileSync(`${src}/${file}`, 'utf8');
            if (!file.endsWith('.svelte')) return { file, code: text };
            const blocks = [...text.matchAll(/<script[^>]*>([\s\S]*?)<\/script>/g)];
            return { file, code: blocks.map((block) => block[1]).join('\n') };
        });
}

/** Each match of `pattern` (global) in the script code, as `file:line: match`. */
function found(pattern: RegExp): string[] {
    return scripts().flatMap(({ file, code }) =>
        code
            .split('\n')
            .flatMap((line, at) =>
                [...line.matchAll(pattern)].map((match) => `${file}:${at + 1}: ${match[0]}`),
            ),
    );
}

describe('the script code of src', () => {
    it('has no colour and no font written by hand', () => {
        // Charts take their colours and their type from the tokens.
        expect.soft(found(/['"`]#[0-9a-fA-F]{3,8}['"`]/g), 'hex colours').toEqual([]);
        expect(found(/\bfont(?:Size|Weight|Family)\s*:\s*['"`\d][^,}]*/g), 'fonts').toEqual([]);
    });

    it('watches data-theme with a MutationObserver only in echarts.ts', () => {
        const watchers = scripts()
            .filter(
                ({ code }) => code.includes('new MutationObserver') && code.includes('data-theme'),
            )
            .map(({ file }) => file);

        expect(watchers).toEqual(['lib/echarts.ts']);
    });
});
