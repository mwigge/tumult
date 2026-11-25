import { cpSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

import { build, resolveTokens } from '../../scripts/build-tokens.mjs';

const WEB = fileURLToPath(new URL('../..', import.meta.url));
const TOKENS = join(WEB, 'tokens');
/** The colour block of the CLI report, as a path under web/. */
const REPORT = '../tumult-cli/src/commands/report/html/tokens.css';
/** The colour variables of the docs theme, as a path under web/. */
const DOCS_SCHEME = '../docs/_sass/color_schemes/_tumult-tokens.scss';
/** The colours of the landing page of the docs site, as a path under web/. */
const LANDING = '../docs/assets/css/tokens.css';

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

    it('takes a percent on a number only, through the Tumult extension', () => {
        const round = ($type: string, $value: unknown, unit: string) => () =>
            resolve({ round: { $type, $value, $extensions: { 'rs.tumult': { unit } } } });

        expect(round('number', 50, '%')().get('round')).toMatchObject({ value: 50, unit: '%' });
        expect(round('dimension', { value: 50, unit: 'px' }, '%')).toThrow(
            'round: only a number takes a unit from the extension rs.tumult',
        );
        expect(round('number', 50, 'em')).toThrow(
            'round: the extension rs.tumult gives a unit other than %',
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
            '--font-size-xs: 12px;',
            '--font-weight-strong: 600;',
            '--line-height-body: 1.5;',
            '--letter-spacing-md: 0.6px;',
            '--space-0-5: 2px;',
            '--space-2: 8px;',
            '--radius-xl: 6px;',
            // A number with the percent of the Tumult extension.
            '--radius-round: 50%;',
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

    it('writes the chart sizes and the chart weight to the TS as numbers, as ECharts takes them', () => {
        const ts = build(TOKENS)['src/lib/tokens.gen.ts'];

        expect(ts).toMatch(
            /"size": \{\s*"2xs": 9,\s*"xs": 10,\s*"sm": 11,\s*"md": 12,\s*"lg": 14\s*\}/,
        );
        expect(ts).toContain('"weight": 600');
    });

    it('writes the colour block of the CLI report: light first, dark when the system asks', () => {
        const css = build(TOKENS)[REPORT];
        const [light, dark, print] = css.split('@media ');

        // A report is read on its own, so light comes first, at :root.
        expect(light).toMatch(/\n:root \{\n {4}color-scheme: light dark;\n {4}--bg: #f4f6f8;\n/);
        expect(light).toContain('\n    --bg-panel: #ffffff;\n');
        expect(light).toContain('\n    --text-dim: #3b4751;\n');
        expect(dark).toMatch(
            /^\(prefers-color-scheme: dark\) \{\n {4}:root \{\n {8}--bg: #0b0e11;\n/,
        );
        expect(dark).toContain('\n        --bg-panel: #151b21;\n');
        expect(dark).toContain('\n        --text-dim: #8b98a5;\n');
        // Paper is white: in print the page has the colour of a light panel.
        expect(print).toBe('print {\n    :root {\n        --bg: #ffffff;\n    }\n}\n');
        // Only colours: the report has its own type and spacing.
        expect(css).not.toMatch(/--(?:mono|font-|line-height|letter-spacing|space-|radius-|z-)/);
    });

    it('writes the colour variables of the docs theme from the light theme', () => {
        const scss = build(TOKENS)[DOCS_SCHEME];

        for (const line of [
            // The docs page is white, as a light panel is. The sidebar and
            // the code panel have the colour of the light page.
            '$body-background-color: #ffffff;',
            '$sidebar-color: #f4f6f8;',
            '$code-background-color: #f4f6f8;',
            '$body-text-color: #1b2329;',
            '$link-color: #1f6fb2;',
            // A colour of the docs only: keywords in code, darker than the fail colour.
            '$tumult-code-keyword: #b5352a;',
        ])
            expect(scss).toContain(`\n${line}\n`);
        // GitHub Pages compiles it with Ruby Sass 3: only comments and plain variables.
        for (const line of scss.split('\n').filter(Boolean))
            expect(line).toMatch(/^(?:\/\/.*|\$[a-z0-9-]+: #[0-9a-fA-F]{6};)$/);
    });

    it('writes the colours of the landing page: light, with a dark terminal', () => {
        const css = build(TOKENS)[LANDING];
        const root = css.match(/^:root \{([^}]*)\}/m)?.[1] ?? '';

        for (const property of [
            '--page: #ffffff;',
            '--band: #f4f6f8;',
            '--text: #1b2329;',
            '--copper: #9a5320;',
            // The terminal is dark on purpose: it takes the dark theme.
            '--term-bg: #0b0e11;',
            '--term-text: #d8dee4;',
        ])
            expect(root).toContain(`\n    ${property}\n`);
        // In Krönika --bg is the grey of the light page, so the landing page
        // has names of its own for its white page and its grey bands.
        expect(root).not.toMatch(/--bg(?:-raised)?:/);
        // One block: the page is light only.
        expect(css.match(/\{/g)).toHaveLength(1);
    });

    it('rejects a token of the docs set that is not a colour', () => {
        const dir = mkdtempSync(join(tmpdir(), 'tokens-'));
        try {
            cpSync(TOKENS, dir, { recursive: true });
            const file = join(dir, 'docs.tokens.json');
            const docs = JSON.parse(readFileSync(file, 'utf8'));
            docs.docs.landing.page = { $type: 'dimension', $value: { value: 4, unit: 'px' } };
            writeFileSync(file, JSON.stringify(docs));

            expect(() => build(dir)).toThrow(
                'docs.landing.page: a token of the docs set must be a colour',
            );
        } finally {
            rmSync(dir, { recursive: true, force: true });
        }
    });

    it('keeps the colours of the docs out of the outputs of Krönika', () => {
        const outputs = build(TOKENS);

        for (const file of ['src/lib/tokens.css', 'src/lib/tokens.gen.ts', REPORT])
            expect(outputs[file], file).not.toMatch(/docs|copper|--term-|--page|--band/i);
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

describe('the docs site', () => {
    const docs = (file: string) => readFileSync(join(WEB, '../docs', file), 'utf8');
    /** The names of the custom properties that a style sheet defines. */
    const defined = (css: string) => [...css.matchAll(/(--[a-z0-9-]+):/g)].map((m) => m[1]);

    it('names every colour of landing.css by a token, with no hex of its own', () => {
        expect(docs('assets/css/landing.css').match(/#[0-9a-fA-F]{3,8}\b/g) ?? []).toEqual([]);
    });

    it('uses on the landing page only the custom properties that its style sheets define', () => {
        const landing = docs('assets/css/landing.css');
        const known = new Set([...defined(docs('assets/css/tokens.css')), ...defined(landing)]);
        const used = [landing, docs('index.html')].flatMap((text) =>
            [...text.matchAll(/var\((--[a-z0-9-]+)/g)].map((m) => m[1]),
        );

        expect([...new Set(used)].filter((name) => !known.has(name))).toEqual([]);
    });

    it('links the colours of the landing page before landing.css', () => {
        const page = docs('index.html');
        const tokens = page.indexOf('<link rel="stylesheet" href="/assets/css/tokens.css">');

        expect(tokens).toBeGreaterThan(-1);
        expect(tokens).toBeLessThan(page.indexOf('/assets/css/landing.css'));
    });

    it('takes the colour variables of the docs theme from the generated partial', () => {
        const scheme = docs('_sass/color_schemes/tumult.scss');

        expect(scheme).toContain('\n@import "./color_schemes/tumult-tokens";\n');
        expect(scheme).not.toMatch(/^\$[a-z0-9-]+:/m);
    });
});
