import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

import { build, resolveTokens } from '../../scripts/build-tokens.mjs';

const WEB = fileURLToPath(new URL('../..', import.meta.url));
const TOKENS = join(WEB, 'tokens');

/** An srgb colour as the DTCG format writes it. */
const colour = (hex: string) => ({
    colorSpace: 'srgb',
    components: [1, 3, 5].map((at) => parseInt(hex.slice(at, at + 2), 16) / 255),
    hex,
});

/** Resolve token trees in the order given, each as a set of its own. */
const resolve = (...trees: Record<string, unknown>[]) =>
    resolveTokens(trees.map((tokens, at) => ({ set: `set-${at}`, tokens })));

describe('resolveTokens', () => {
    it('gives an alias the value and the type of the token it names', () => {
        const tokens = resolve(
            { grey: { $type: 'color', 4: { $value: colour('#0b0e11') } } },
            { bg: { $value: '{grey.4}' }, page: { $type: 'color', $value: '{bg}' } },
        );

        expect(tokens.get('bg')).toMatchObject({ type: 'color', value: colour('#0b0e11') });
        expect(tokens.get('page')).toMatchObject({ type: 'color', value: colour('#0b0e11') });
    });

    it('takes the last token of a path when two sets name it', () => {
        const tokens = resolve(
            { bg: { $type: 'color', $value: colour('#0b0e11') } },
            { bg: { $type: 'color', $value: colour('#f4f6f8') } },
        );

        expect(tokens.get('bg')).toMatchObject({ value: colour('#f4f6f8'), set: 'set-1' });
    });

    it('rejects an alias that names no token', () => {
        expect(() =>
            resolve(
                { grey: { $type: 'color', 4: { $value: colour('#0b0e11') } } },
                { bg: { $type: 'color', $value: '{grey.5}' } },
            ),
        ).toThrow('bg: the alias {grey.5} names no token');
    });

    it('rejects aliases that go round in a loop', () => {
        expect(() =>
            resolve({
                a: { $type: 'color', $value: '{b}' },
                b: { $type: 'color', $value: '{a}' },
            }),
        ).toThrow('a: the aliases go round in a loop: a -> b -> a');
        expect(() =>
            resolve({
                start: { $type: 'color', $value: '{a}' },
                a: { $type: 'color', $value: '{b}' },
                b: { $type: 'color', $value: '{a}' },
            }),
        ).toThrow('a: the aliases go round in a loop: a -> b -> a');
    });

    it('rejects a token with no type of its own, of a group or of the token it names', () => {
        expect(() => resolve({ bg: { $value: colour('#0b0e11') } })).toThrow(
            'bg: no $type on the token, on a group above it or on the token it names',
        );
    });

    it('rejects a colour token whose value is not a colour', () => {
        const not = 'bg: $type is color, but the value is not a colour';
        expect(() => resolve({ bg: { $type: 'color', $value: '#0b0e11' } })).toThrow(not);
        expect(() =>
            resolve({ bg: { $type: 'color', $value: { ...colour('#0b0e11'), hex: '#0b0e12' } } }),
        ).toThrow(`${not}: its components are not its hex`);
        expect(() =>
            resolve({
                font: { $type: 'fontFamily', $value: ['Menlo', 'monospace'] },
                bg: { $type: 'color', $value: '{font}' },
            }),
        ).toThrow(not);
    });

    it('rejects a dimension with no unit, or with a unit other than px and rem', () => {
        const not = 'space: $type is dimension, but the value is not a dimension';
        const space = ($value: unknown) => () => resolve({ space: { $type: 'dimension', $value } });

        expect(space({ value: 4 })).toThrow(`${not}: it has no unit`);
        expect(space(4)).toThrow(`${not}: it has no unit`);
        expect(space('4px')).toThrow(`${not}: it is not an object with a value and a unit`);
        expect(space({ value: 4, unit: 'em' })).toThrow(`${not}: its unit is not px or rem`);
        expect(space({ value: '4', unit: 'px' })).toThrow(`${not}: its value is not a number`);
        expect(space({ value: 0.5, unit: 'rem' })()).toBeInstanceOf(Map);
    });

    it('rejects a font weight outside 1 to 1000', () => {
        const not = 'weight: $type is fontWeight, but the value is not a number from 1 to 1000';
        const weight = ($value: unknown) => () =>
            resolve({ weight: { $type: 'fontWeight', $value } });

        for (const value of [0, 1001, '600']) expect(weight(value), String(value)).toThrow(not);
        for (const value of [1, 600, 1000])
            expect(weight(value)(), String(value)).toBeInstanceOf(Map);
    });

    it('rejects a number token whose value is not a number', () => {
        expect(() => resolve({ z: { $type: 'number', $value: '20' } })).toThrow(
            'z: $type is number, but the value is not a number',
        );
    });
});

describe('build', () => {
    it('writes the default theme at :root and each other theme for data-theme and auto', () => {
        const css = build(TOKENS)['src/lib/tokens.css'];

        expect(css).toMatch(/^:root \{\n {4}color-scheme: dark;\n[^}]* {4}--bg: #0b0e11;\n/m);
        expect(css).toContain(":root[data-theme='light'] {\n    color-scheme: light;\n");
        expect(css).toContain(
            "@media (prefers-color-scheme: light) {\n    :root[data-theme='auto'] {\n        color-scheme: light;\n",
        );
        expect(css).toContain(
            "--mono: 'SF Mono', 'JetBrains Mono', ui-monospace, Menlo, Consolas, monospace;",
        );
        // A theme other than the default holds only what it changes.
        expect(css.split(":root[data-theme='light']")[1]).not.toContain('--mono');
    });

    it('writes the type, spacing, radius, border, layer and drawer tokens once, at :root', () => {
        const css = build(TOKENS)['src/lib/tokens.css'];
        const root = css.match(/^:root \{([^}]*)\}/m)?.[1] ?? '';

        for (const property of [
            // A fontFamily. A name of one word has no quotes, also when it starts with a dash.
            "--font-body: -apple-system, 'Segoe UI', Inter, Roboto, sans-serif;",
            // Dimensions, a fontWeight and numbers.
            '--font-size-sm: 12.5px;',
            '--font-weight-strong: 600;',
            '--line-height-body: 1.5;',
            '--letter-spacing-md: 0.6px;',
            '--space-1-5: 6px;',
            '--space-2: 8px;',
            '--radius-lg: 5px;',
            '--border-width-sm: 1px;',
            '--z-sticky: 2;',
            '--drawer-narrow: 480px;',
        ])
            expect(root).toContain(`\n    ${property}\n`);
        // The same in each theme, so no other block holds them.
        expect(css.split(":root[data-theme='light']")[1]).not.toMatch(
            /--(?:font-|line-height|letter-spacing|space-|radius-|border-width|z-|drawer-)/,
        );
    });

    it('keeps the type, spacing, layer and drawer tokens out of the TS, which the charts read', () => {
        // The script of the CLI report bundles tokens.gen.ts, and it draws no page.
        expect(build(TOKENS)['src/lib/tokens.gen.ts']).not.toMatch(
            /"(?:font-|line-height|letter-spacing|space-|radius-|border-width|z-|drawer-)/,
        );
    });

    it('writes the committed outputs from the committed token files', () => {
        for (const [file, text] of Object.entries(build(TOKENS))) {
            expect(
                readFileSync(join(WEB, file), 'utf8'),
                `${file} is not what npm run build:tokens writes`,
            ).toBe(text);
        }
    });
});

describe('theme.css', () => {
    it('takes its colours from tokens.css and defines none of its own', () => {
        const theme = readFileSync(join(WEB, 'src/lib/theme.css'), 'utf8');

        expect(theme.startsWith("@import './tokens.css';\n")).toBe(true);
        expect(theme).not.toMatch(/--[a-z-]+:/);
    });
});
