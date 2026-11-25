import { readdirSync, readFileSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

import { tokens } from './tokens.gen';

// WCAG 2.2 AA contrast in both themes (plan decision D6), from the colour
// tokens that the pages and the charts draw with. Text needs 4.5:1. Large
// text, and the parts of the UI and of charts that a user needs to see,
// need 3:1. A failing check lists each pair under its bar, with its ratio.

const SRC = fileURLToPath(new URL('..', import.meta.url));

type Theme = keyof typeof tokens;
const THEMES = Object.keys(tokens) as Theme[];

/** The surfaces that text and controls sit on. */
const SURFACES = ['bg', 'bg-raised', 'bg-panel', 'bg-hover'] as const;
/** The colours of text. Each is used for small text, so each needs 4.5:1 on every surface. */
const TEXT = ['text', 'text-dim', 'text-faint', 'ok', 'warn', 'fail', 'accent'] as const;
/** The fill of the selected segment and of the primary button, and the text on it. */
const FILL = 'accent-dim';
const ON_FILL = 'on-accent';
/** The edge of a field: an input, a select or a text area. */
const FIELD_EDGE = 'border-input';
/** The focus ring: the edge of a field with the focus, and the outline of a control. */
const FOCUS = 'accent';
/**
 * Lines that WCAG 1.4.11 does not ask 3:1 of. It asks it only of what a
 * user needs to find a control or its state, and of graphics a user needs
 * to understand the content.
 */
const EXEMPT: Record<string, string> = {
    border: 'the edge of a panel, the line between rows and the grid of a chart: a panel is found by its content and its fill, a row by its text, a value by its axis label',
    'border-strong':
        'the line under a table head, the axis of a chart, and the edge of a button, a chip and a drawer: a button and a chip are named by their text, and an axis by its labels',
};
/**
 * The chart roles. The band colours are text only in the large score of
 * the Scores page (24 px, semi-bold), and their glyphs and sparklines are
 * graphics, so their bar is 3:1. No other colour is used for large text only.
 */
const CHART = [
    'font',
    'series',
    'extra-series',
    'band',
    'unscored',
    'status',
    'ink-on-fill',
    'neutral-dot',
    'starmap',
];
/** Rules with the accent fill that hold no text: a bar of a chart in a table. */
const NO_TEXT = [
    { file: join('routes', '+page.svelte'), selector: '.tbar' },
    { file: join('routes', 'traces', '+page.svelte'), selector: '.bar' },
];

/** The relative luminance of a colour `#rrggbb`, as WCAG 2.2 counts it. */
function luminance(hex: string): number {
    const channel = (at: number) => {
        const value = parseInt(hex.slice(at, at + 2), 16) / 255;
        return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
    };
    return 0.2126 * channel(1) + 0.7152 * channel(3) + 0.0722 * channel(5);
}

/** The contrast ratio of two colours, from 1 to 21. */
function contrast(a: string, b: string): number {
    const [light, dark] = [luminance(a), luminance(b)].sort((x, y) => y - x);
    return (light + 0.05) / (dark + 0.05);
}

/** `percent` of `colour` over `base`, as `color-mix(in srgb, ...)` paints it. */
function mix(colour: string, percent: number, base: string): string {
    const part = (hex: string, at: number) => parseInt(hex.slice(at, at + 2), 16);
    const share = percent / 100;
    const bytes = [1, 3, 5].map((at) =>
        Math.round(part(colour, at) * share + part(base, at) * (1 - share)),
    );
    return `#${bytes.map((byte) => byte.toString(16).padStart(2, '0')).join('')}`;
}

interface Pair {
    what: string;
    fg: string | undefined;
    bg: string | undefined;
    bar: number;
}

/** Each pair under its bar, with its ratio, and each pair that names no colour. */
const below = (pairs: Pair[]) =>
    pairs.flatMap(({ what, fg, bg, bar }) => {
        if (fg === undefined || bg === undefined) return [`${what}: no such colour role`];
        const ratio = contrast(fg, bg);
        return ratio >= bar ? [] : [`${what}: ${fg} on ${bg} is ${ratio.toFixed(2)}, needs ${bar}`];
    });

/** The colour roles of a theme that CSS reads: name to hex. */
const roles = (theme: Theme): Record<string, string> =>
    Object.fromEntries(
        Object.entries(tokens[theme]).filter(
            (entry): entry is [string, string] =>
                typeof entry[1] === 'string' && entry[1].startsWith('#'),
        ),
    );

/** Each colour of a group of chart tokens, by its dotted name. */
const colours = (group: object, name: string): [string, string][] =>
    Object.entries(group).flatMap(([key, value]) =>
        typeof value === 'string'
            ? [[`${name}.${key}`, value] as [string, string]]
            : colours(value as object, `${name}.${key}`),
    );

interface Rule {
    file: string;
    selectors: string[];
    declarations: string[];
}

/** The style rules of CSS text; the rules inside @media count. */
function rulesOf(file: string, css: string): Rule[] {
    const text = css.replace(/\/\*[\s\S]*?\*\//g, '');
    return [...text.matchAll(/([^{};]+)\{([^{}]*)\}/g)]
        .filter(([, prelude]) => !prelude.trim().startsWith('@'))
        .map(([, prelude, body]) => ({
            file,
            selectors: prelude.split(',').map((selector) => selector.trim().replace(/\s+/g, ' ')),
            declarations: body
                .split(';')
                .map((declaration) => declaration.trim().replace(/\s+/g, ' '))
                .filter((declaration) => declaration !== ''),
        }));
}

/** The Svelte files under `dir`, as paths from src/. */
function svelteFiles(dir: string): string[] {
    return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
        const path = join(dir, entry.name);
        if (entry.isDirectory()) return svelteFiles(path);
        return entry.name.endsWith('.svelte') ? [relative(SRC, path)] : [];
    });
}

const read = (path: string) => readFileSync(join(SRC, path), 'utf8');
const THEME_CSS = join('lib', 'theme.css');
const SCORES = join('routes', 'scores', '+page.svelte');
/** Every style rule of theme.css and of the <style> blocks of the Svelte files. */
const RULES = [
    ...rulesOf(THEME_CSS, read(THEME_CSS)),
    ...svelteFiles(SRC).flatMap((file) =>
        rulesOf(
            file,
            [...read(file).matchAll(/<style[^>]*>([\s\S]*?)<\/style>/g)]
                .map((block) => block[1])
                .join('\n'),
        ),
    ),
];

/** The value of `property` in `rule`, or undefined. */
const valueOf = (rule: Rule, property: string) =>
    rule.declarations
        .find((declaration) => declaration.startsWith(`${property}:`))
        ?.slice(property.length + 1)
        .trim();

/** The role a value names: `var(--ok)` names ok. */
const roleIn = (value: string | undefined) => /^var\(--([a-z-]+)\)$/.exec(value ?? '')?.[1];

/**
 * The pairs of the text of each badge and its tint. A tint mixed with
 * `transparent` takes the colour of the surface under it, so it is read
 * on each surface.
 */
function badgePairs(theme: Theme): Pair[] {
    const role = roles(theme);
    return RULES.filter((rule) => rule.file === THEME_CSS)
        .flatMap((rule) => rule.selectors.map((selector) => ({ rule, selector })))
        .filter(({ selector }) => /^\.badge\.(?:ok|warn|fail|neutral)$/.test(selector))
        .flatMap(({ rule, selector }) => {
            const text = role[roleIn(valueOf(rule, 'color')) ?? ''];
            const background = valueOf(rule, 'background') ?? '';
            const tint =
                /^color-mix\(in srgb, var\(--([a-z-]+)\) (\d+)%, (?:var\(--([a-z-]+)\)|transparent)\)$/.exec(
                    background,
                );
            if (!tint) {
                const fill = role[roleIn(background) ?? ''];
                return [{ what: `${selector} on its fill`, fg: text, bg: fill, bar: 4.5 }];
            }
            const [, tone, percent, base] = tint;
            const bases = base === undefined ? SURFACES : [base];
            return bases.map((under) => ({
                what: `${selector} on its tint over ${under}`,
                fg: text,
                bg: mix(role[tone], Number(percent), role[under]),
                bar: 4.5,
            }));
        });
}

describe('the contrast of the colour roles', () => {
    for (const theme of THEMES) {
        const role = roles(theme);
        const chart = tokens[theme].chart;
        const panel = role['bg-panel'];
        const on = (names: readonly string[], surfaces: readonly string[], bar: number) =>
            names.flatMap((name) =>
                surfaces.map((surface) => ({
                    what: `${name} on ${surface}`,
                    fg: role[name],
                    bg: role[surface],
                    bar,
                })),
            );
        const onPanel = (group: object, name: string, bar: number) =>
            colours(group, name).map(([what, colour]) => ({
                what: `${what} on bg-panel`,
                fg: colour,
                bg: panel,
                bar,
            }));

        it(`gives every text colour 4.5:1 on every surface, ${theme} theme`, () => {
            expect(below(on(TEXT, SURFACES, 4.5))).toEqual([]);
        });

        it(`gives the text on the accent fill 4.5:1, ${theme} theme`, () => {
            expect(below(on([ON_FILL], [FILL], 4.5))).toEqual([]);
        });

        it(`sets the accent fill 3:1 off the page and the panel, ${theme} theme`, () => {
            expect(below(on([FILL], ['bg', 'bg-panel'], 3))).toEqual([]);
        });

        it(`sets the edge of a field 3:1 off its fill and the panel, ${theme} theme`, () => {
            expect(below(on([FIELD_EDGE], ['bg-raised', 'bg-panel'], 3))).toEqual([]);
        });

        it(`sets the focus ring 3:1 off every surface, ${theme} theme`, () => {
            expect(below(on([FOCUS], SURFACES, 3))).toEqual([]);
        });

        it(`gives the text of each badge 4.5:1 on its tint, ${theme} theme`, () => {
            const pairs = badgePairs(theme);

            expect(pairs.length).toBeGreaterThanOrEqual(4);
            expect(below(pairs)).toEqual([]);
        });

        it(`draws every chart series 3:1 or more on the panel, ${theme} theme`, () => {
            const series = [
                ...onPanel(chart.series, 'series', 3),
                ...onPanel(chart['extra-series'], 'extra-series', 3),
            ];

            expect(below(series)).toEqual([]);
        });

        it(`draws the bands and the evidence status 3:1 or more on the panel, ${theme} theme`, () => {
            const bands = [
                ...onPanel(chart.band, 'band', 3),
                ...onPanel({ unscored: chart.unscored }, 'chart', 3),
                ...onPanel(chart.status, 'status', 3),
                ...onPanel({ 'neutral-dot': chart['neutral-dot'] }, 'chart', 3),
            ];

            expect(below(bands)).toEqual([]);
        });

        it(`writes the label on a band, a no-score tile and a status cell 4.5:1, ${theme} theme`, () => {
            const fills: [string, string][] = [
                ...colours(chart.band, 'band'),
                ['unscored', chart.unscored],
                ...colours(chart.status, 'status'),
            ];
            const labels = fills.map(([name, fill]) => ({
                what: `ink-on-fill on ${name}`,
                fg: chart['ink-on-fill'],
                bg: fill,
                bar: 4.5,
            }));

            expect(below(labels)).toEqual([]);
        });

        it(`draws every Star Map node and relation 3:1 or more on the panel, ${theme} theme`, () => {
            const starmap = [
                ...onPanel(chart.starmap.kind, 'kind', 3),
                ...onPanel(chart.starmap.relation, 'relation', 3),
            ];

            expect(below(starmap)).toEqual([]);
        });

        it(`checks every colour role, or says why it needs no check, ${theme} theme`, () => {
            const checked = [
                ...SURFACES,
                ...TEXT,
                FILL,
                ON_FILL,
                FIELD_EDGE,
                ...Object.keys(EXEMPT),
            ];

            expect(Object.keys(role).filter((name) => !checked.includes(name))).toEqual([]);
            expect(Object.keys(chart).filter((name) => !CHART.includes(name))).toEqual([]);
        });
    }
});

/** The CIE lightness L* of a colour `#rrggbb`, from 0 for black to 100 for white. */
function lightness(hex: string): number {
    const y = luminance(hex);
    return y > 216 / 24389 ? 116 * Math.cbrt(y) - 16 : (24389 / 27) * y;
}

describe('the text colours of the light theme', () => {
    // text-faint is the lightest grey that is 4.5:1 on bg-hover, the darkest
    // light surface. Halfway between it and text, text-dim is 16 L* from
    // each: the most that AA leaves for both steps.
    const STEP = 15;

    it(`stand ${STEP} L* or more apart, so that each level reads as its own`, () => {
        const role = roles('light');
        const steps = [
            ['text', 'text-dim'],
            ['text-dim', 'text-faint'],
        ].flatMap(([darker, lighter]) => {
            const step = lightness(role[lighter]) - lightness(role[darker]);
            return step >= STEP
                ? []
                : [`${darker} to ${lighter}: ${step.toFixed(1)} L*, needs ${STEP}`];
        });
        expect(steps).toEqual([]);
    });
});

describe('the styles that draw on a colour role', () => {
    it('write the large score of the Scores page, the one text in a band colour, 24 px or more', () => {
        // A band is 3:1 on the panel. WCAG lets large text have 3:1: 24 px,
        // or 18.66 px when it is bold.
        const score = RULES.find(
            (rule) => rule.file === SCORES && rule.selectors.includes('.card .value'),
        );
        const token = /^var\(--([a-z0-9-]+)\)$/.exec(valueOf(score ?? RULES[0], 'font-size') ?? '');
        const size = new RegExp(`--${token?.[1]}: (\\d+(?:\\.\\d+)?)px;`).exec(
            read(join('lib', 'tokens.css')),
        );

        expect(score).toBeDefined();
        expect(Number(size?.[1])).toBeGreaterThanOrEqual(24);
    });

    it('write the text on the accent fill in --on-accent', () => {
        const filled = RULES.filter((rule) =>
            ['background', 'background-color'].some(
                (property) => roleIn(valueOf(rule, property)) === FILL,
            ),
        );
        const wrong = filled
            .filter(
                (rule) =>
                    !NO_TEXT.some(
                        ({ file, selector }) =>
                            rule.file === file && rule.selectors.includes(selector),
                    ),
            )
            .filter((rule) => roleIn(valueOf(rule, 'color')) !== ON_FILL)
            .map((rule) => `${rule.file}: ${rule.selectors.join(', ')}`);

        expect(filled.length).toBeGreaterThanOrEqual(NO_TEXT.length + 3);
        expect(wrong).toEqual([]);
    });

    it('draw the edge of a field in --border-input, or the focus ring', () => {
        const field = /(?:^|[\s>+~(])(?:input|select|textarea)\b/;
        const edges = RULES.filter((rule) => rule.selectors.some((s) => field.test(s))).flatMap(
            (rule) =>
                rule.declarations
                    .filter((declaration) =>
                        /^border(?:-(?:top|right|bottom|left))?(?:-color)?:/.test(declaration),
                    )
                    .flatMap((declaration) =>
                        [...declaration.matchAll(/var\(--([a-z-]+)\)/g)]
                            .map((match) => match[1])
                            .filter((name) => !name.startsWith('border-width'))
                            .map((name) => ({ rule, name })),
                    ),
        );
        const wrong = edges
            .filter(({ name }) => name !== FIELD_EDGE && name !== FOCUS)
            .map(({ rule, name }) => `${rule.file}: ${rule.selectors.join(', ')} draws --${name}`);

        expect(edges.length).toBeGreaterThanOrEqual(7);
        expect(wrong).toEqual([]);
    });
});
