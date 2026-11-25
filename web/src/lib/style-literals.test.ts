import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

import { KINDS, literalsIn, scan } from '../../scripts/style-literals.mjs';

const WEB = fileURLToPath(new URL('../..', import.meta.url));

/**
 * The raw values that the pages keep, by kind: values used fewer than 3
 * times, odd and negative spacing. They have no token yet. A later change
 * moves them onto the scale, so a count may only go down.
 */
const LEFT: Record<string, number> = {
    'font-size': 11,
    'font-weight': 4,
    'line-height': 2,
    'letter-spacing': 4,
    'font-family': 0,
    spacing: 51,
    radius: 5,
    'border-width': 2,
    'z-index': 0,
};

/** [kind, value, line] of each raw value of a file. */
const found = (file: string, text: string) =>
    literalsIn(file, text).map(({ kind, value, line }) => [kind, value, line]);

describe('the scan of raw style values', () => {
    it('reads the style blocks and style attributes of a Svelte file, with their lines', () => {
        const svelte = [
            '<script>let w = 1;</script>',
            '<p style="margin-top: 14px; width: {w}px; padding: {w}px 6px">x</p>',
            '<style>',
            '    p { font-size: 12.5px; }',
            '    @media (max-width: 900px) { p { gap: 8px } }',
            '</style>',
        ].join('\n');

        expect(found('src/x.svelte', svelte)).toEqual([
            ['font-size', '12.5px', 4],
            ['spacing', '8px', 5],
            ['spacing', '14px', 2],
            ['spacing', '6px', 2],
        ]);
    });

    it('reads every declaration of a CSS file', () => {
        const css = 'a:hover {\n    border: 1px solid var(--accent);\n    z-index: 20;\n}\n';

        expect(found('src/x.css', css)).toEqual([
            ['border-width', '1px', 2],
            ['z-index', '20', 3],
        ]);
    });

    it('leaves out values in var(), zero, keywords, custom properties and comments', () => {
        const css = [
            'p {',
            '    margin: 0 auto var(--space-2, 8px);',
            '    font: inherit;',
            '    font-weight: normal;',
            '    font-family: var(--mono, monospace);',
            '    --gap: 10px;',
            '    /* padding: 4px; */',
            '}',
        ].join('\n');

        expect(found('src/x.css', css)).toEqual([]);
    });

    it('splits a font shorthand into its size, line height and family', () => {
        const css = "body { font: 14px/1.5 -apple-system, 'Segoe UI', sans-serif; }";

        expect(found('src/x.css', css)).toEqual([
            ['font-size', '14px', 1],
            ['line-height', '1.5', 1],
            ['font-family', "-apple-system, 'Segoe UI', sans-serif", 1],
        ]);
    });

    it('counts a negative margin, each side of a shorthand and only the width of a border', () => {
        const css = [
            'p {',
            '    margin: -4px 0 3px;',
            '    border-radius: 4px 0 0 4px;',
            '    border: 1px solid color-mix(in srgb, var(--ok) 50%, transparent);',
            '}',
        ].join('\n');

        expect(found('src/x.css', css)).toEqual([
            ['spacing', '-4px', 2],
            ['spacing', '3px', 2],
            ['radius', '4px', 3],
            ['radius', '4px', 3],
            ['border-width', '1px', 4],
        ]);
    });
});

describe('the styles of the pages', () => {
    const literals = scan(WEB);

    it('reads the pages, the components and theme.css, but not tokens.css', () => {
        const files = new Set(literals.map(({ file }) => file));

        expect(files).toContain('src/lib/theme.css');
        expect([...files].some((file) => file.startsWith('src/routes/'))).toBe(true);
        expect([...files].some((file) => file.startsWith('src/lib/components/'))).toBe(true);
        expect(files).not.toContain('src/lib/tokens.css');
    });

    for (const kind of KINDS) {
        it(`keep only the raw ${kind} values that have no token`, () => {
            const mine = literals.filter((literal) => literal.kind === kind);
            const where = mine.map((l) => `${l.file}:${l.line} ${l.property}: ${l.value}`);

            expect(mine.length, `the raw ${kind} values:\n${where.join('\n')}`).toBe(LEFT[kind]);
        });
    }
});
