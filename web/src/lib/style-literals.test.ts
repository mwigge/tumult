import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

import { FIX, lint, lintStyles, literalsIn, scan } from '../../scripts/style-literals.mjs';

const WEB = fileURLToPath(new URL('../..', import.meta.url));

/**
 * A small tokens.css: a spacing and a radius token, a colour that is the
 * same in each theme and a colour that is not.
 */
const TOKENS = [
    ':root {',
    '    --space-2: 8px;',
    '    --radius-2xl: 10px;',
    '    --on-accent: #ffffff;',
    '    --bg: #0b0e11;',
    '}',
    ":root[data-theme='light'] {",
    '    --bg: #f4f6f8;',
    '}',
].join('\n');

/** A page that uses spacing 3px twice, spacing 10px and radius 10px. */
const PAGE = [
    '<p style="margin: 3px">x</p>',
    '<style>',
    '    p { padding: 3px; gap: 10px; }',
    '    b { border-radius: 10px; }',
    '</style>',
].join('\n');

/** The allow list that holds each raw value of PAGE. */
const ALLOWED = [
    '# The raw values left.',
    '',
    'src/x.svelte spacing 3px',
    'src/x.svelte spacing 3px',
    'src/x.svelte spacing 10px',
    'src/x.svelte radius 10px',
];

/** The problems that the lint finds in `page`, with `allow` as the allow list. */
const problems = (allow: string[], page = PAGE) =>
    lint(literalsIn('src/x.svelte', page), allow.join('\n'), TOKENS);

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

    it('finds raw colours: hex, rgb(), hsl() and named colours', () => {
        const svelte = [
            '<p style="color: red; background: {tone}">x</p>',
            '<style>',
            '    p { border: 1px solid #fff; box-shadow: 0 2px 4px rgb(0 0 0 / 25%); }',
            '    b { background: hsl(210, 40%, 50%) url(#fade); color: RebeccaPurple; }',
            '</style>',
        ].join('\n');

        expect(found('src/x.svelte', svelte)).toEqual([
            ['border-width', '1px', 3],
            ['colour', '#fff', 3],
            ['colour', 'rgb(0 0 0 / 25%)', 3],
            ['colour', 'hsl(210, 40%, 50%)', 4],
            ['colour', 'RebeccaPurple', 4],
            ['colour', 'red', 1],
        ]);
    });

    it('leaves out transparent, currentColor, inherit and colours in var() or a string', () => {
        const css = [
            'p {',
            '    color: currentColor;',
            '    border-color: inherit;',
            '    outline-color: var(--accent, #5eb1ef);',
            '    background: color-mix(in srgb, var(--ok) 50%, transparent);',
            "    content: 'red';",
            '    transition: color 0.2s;',
            '}',
        ].join('\n');

        expect(found('src/x.css', css)).toEqual([]);
    });

    it('finds a raw colour in a custom property, but no other raw value there', () => {
        const css = 'p {\n    --tone: #f59e0b;\n    --gap: 10px;\n}\n';

        expect(found('src/x.css', css)).toEqual([['colour', '#f59e0b', 2]]);
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

    it('keep only the raw values on the allow list, and the list has no stale line', () => {
        expect(lintStyles(WEB), FIX).toEqual([]);
    });
});

describe('the style lint', () => {
    it('passes when the allow list holds each raw value as often as it is used', () => {
        expect(problems(ALLOWED)).toEqual([]);
    });

    it('fails on a raw value that is not on the list, and names its file, line, kind and value', () => {
        const allow = ALLOWED.filter((line) => line !== 'src/x.svelte spacing 10px');

        expect(problems(allow)).toEqual([
            'src/x.svelte:3: raw spacing 10px is not on the allow list',
        ]);
    });

    it('names the token of the same kind that has the exact value', () => {
        const page = '<style>\n    p { gap: 8px; border-radius: 8px; margin: 10px; }\n</style>';

        expect(problems([], page)).toEqual([
            'src/x.svelte:2: raw spacing 8px is not on the allow list; use var(--space-2)',
            'src/x.svelte:2: raw radius 8px is not on the allow list',
            'src/x.svelte:2: raw spacing 10px is not on the allow list',
        ]);
    });

    it('counts repeats, so one more use of an allowed value fails', () => {
        const allow = ALLOWED.filter(
            (line, at) => at !== ALLOWED.indexOf('src/x.svelte spacing 3px'),
        );

        expect(problems(allow)).toEqual([
            'src/x.svelte:3: raw spacing 3px is used 2 times and the allow list holds 1 (lines 1, 3)',
        ]);
    });

    it('fails on a line that matches no raw value, so the list only shrinks', () => {
        const allow = [...ALLOWED, 'src/x.svelte spacing 7px', 'src/x.svelte spacing 3px'];

        expect(problems(allow)).toEqual([
            'style-literals-allow.txt:7: src/x.svelte spacing 7px matches no raw value; delete the line',
            'style-literals-allow.txt:8: src/x.svelte spacing 3px matches no raw value; delete the line',
        ]);
    });

    it('fails on a line that is not a path, a known kind and a value', () => {
        const allow = [...ALLOWED, 'src/x.svelte 3px', 'src/x.svelte spaces 3px'];

        expect(problems(allow)).toEqual([
            'style-literals-allow.txt:7: "src/x.svelte 3px" is not a path, a kind and a value',
            'style-literals-allow.txt:8: "src/x.svelte spaces 3px" is not a path, a kind and a value',
        ]);
    });

    it('fails on a raw colour that is not on the list', () => {
        const page = '<style>\n    p { background: rgba(0, 0, 0, 0.45); }\n</style>';

        expect(problems(ALLOWED, page)).toContain(
            'src/x.svelte:2: raw colour rgba(0, 0, 0, 0.45) is not on the allow list',
        );
    });

    it('names a colour token only when it has the colour in every theme', () => {
        const page = '<style>\n    p { color: #FFF; background: #0b0e11; }\n</style>';

        expect(problems([], page)).toEqual([
            'src/x.svelte:2: raw colour #FFF is not on the allow list; use var(--on-accent)',
            'src/x.svelte:2: raw colour #0b0e11 is not on the allow list',
        ]);
    });

    it('says how to fix a failure in one line', () => {
        expect(FIX).not.toContain('\n');
        expect(FIX).toContain('tokens.css');
        expect(FIX).toContain('style-literals-allow.txt');
    });
});
