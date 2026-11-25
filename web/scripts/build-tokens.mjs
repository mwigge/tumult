#!/usr/bin/env node
// Write the outputs of the design tokens of Krönika from web/tokens/, which
// hold DTCG 2025.10 JSON: tumult.resolver.json names the sets and the theme
// modifier. For each theme this merges the sets in order, resolves every
// alias and checks every value. It writes src/lib/tokens.css (the custom
// properties of each theme), src/lib/tokens.gen.ts (the same values for
// ECharts, which cannot read CSS), the colours of the CLI HTML report, the
// colours of the docs site and the pictures of the tokens that the UI
// standard page of the docs shows. All are committed, and CI fails when they
// are not what this writes. It reads only the part of the format that
// Tumult uses: srgb colours with a hex, font families, dimensions in px or
// rem, font weights and numbers as numbers, aliases, sets and one modifier.
// The format has no percent, so a number may take the unit % from the
// extension of Tumult: `"$extensions": { "rs.tumult": { "unit": "%" } }`.
//
// Usage: node scripts/build-tokens.mjs

import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { basename, dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const WEB = join(dirname(fileURLToPath(import.meta.url)), '..');
const ALIAS = /^\{([^{}]+)\}$/;
// The set of raw values. Pages and charts use roles, so no output has them.
const PRIMITIVE = 'primitive';
// The set of the type, spacing and layer roles. Only pages use them, so only
// the CSS has them: the script of the CLI report bundles the TS.
const PAGE_ONLY = 'page';
// The units of a dimension in the format. CSS writes them the same way.
const UNITS = ['px', 'rem'];
// The extension of Tumult in a token, and the one unit it may give a number.
const EXTENSION = 'rs.tumult';
const PERCENT = '%';
// Charts draw on a canvas and cannot read CSS, so this group is only in the TS.
const CANVAS_ONLY = 'chart.';
// The colours of the HTML report of `tumult report`, as a path under web/.
const REPORT_CSS = '../tumult-cli/src/commands/report/html/tokens.css';
// The set of the colours of the docs site. Jekyll builds the site and cannot
// run this script, so only the files below have them.
const DOCS_ONLY = 'docs';
// The colour variables of the docs theme and the colours of the landing
// page, as paths under web/.
const DOCS_SCHEME = '../docs/_sass/color_schemes/_tumult-tokens.scss';
const LANDING_CSS = '../docs/assets/css/tokens.css';
// The pictures of the tokens on docs/reference/ui-standard.md, as a path
// under web/: a swatch of each colour role in each theme, a strip of the
// series of the charts and the type, spacing and radius scale.
const PICTURES = '../docs/images/3.0/tokens';
/** @typedef {{ text: string, fill: string, edge: string, font: string }} Look */
/**
 * @typedef {{ title: string, prefix: string, row: (px: number) => number,
 *   draw: (x: number, top: number, base: number, px: number, look: Look) => string }} Group
 */
// The groups of page tokens that the scale picture draws: the start of their
// names, the height of a row, and the step drawn at its size at x, in a row
// from `top` whose text stands on `base`.
/** @type {Group[]} */
const SCALE = [
    {
        title: 'Type',
        prefix: 'font-size-',
        row: (px) => Math.max(Math.ceil(px * 1.6), 24),
        draw: (x, _top, base, px, { text, font }) =>
            `<text x="${x + 200}" y="${base}" font-family="${font}" fill="${text}" font-size="${px}">Krönika shows a run</text>`,
    },
    {
        title: 'Spacing',
        prefix: 'space-',
        row: () => 20,
        draw: (x, _top, base, px, { fill }) =>
            `<rect x="${x + 130}" y="${base - 10}" width="${px}" height="12" fill="${fill}"/>`,
    },
    {
        title: 'Radius',
        prefix: 'radius-',
        row: () => 44,
        draw: (x, top, _base, px, { edge }) =>
            `<rect x="${x + 150.5}" y="${top + 4.5}" width="35" height="35" rx="${px}" fill="none" stroke="${edge}"/>`,
    },
];

/** @typedef {{ value: unknown, type?: string, set: string, unit?: string }} Token */
/** @typedef {{ set: string, tokens: Record<string, unknown> }} Source */
/** @typedef {{ $ref: string }} Ref */
/**
 * @typedef {{ version: string, sets: Record<string, { sources: Ref[] }>,
 *   modifiers: { theme: { contexts: Record<string, Ref[]>, default: string } },
 *   resolutionOrder: Ref[] }} Resolver
 */

/** @type {(path: string, text: string) => never} */
const fail = (path, text) => {
    throw new Error(`${path}: ${text}`);
};

/** @param {unknown} error */
const message = (error) => (error instanceof Error ? error.message : String(error));

/**
 * Add the tokens of `tree` to `into` by their dotted path. A token without a
 * `$type` takes the one of the closest group above it.
 * @param {Record<string, unknown>} tree
 * @param {string} set
 * @param {Map<string, Token>} into
 * @param {string[]} [path]
 * @param {string} [type]
 */
function flatten(tree, set, into, path = [], type = undefined) {
    const own = typeof tree.$type === 'string' ? tree.$type : type;
    if ('$value' in tree) {
        const unit = unitOf(tree.$extensions);
        into.set(path.join('.'), {
            value: tree.$value,
            type: own,
            set,
            ...(unit === undefined ? {} : { unit }),
        });
        return;
    }
    for (const [name, child] of Object.entries(tree)) {
        if (name.startsWith('$') || child === null || typeof child !== 'object') continue;
        flatten(/** @type {Record<string, unknown>} */ (child), set, into, [...path, name], own);
    }
}

/**
 * The unit that the extension of Tumult gives a token, or undefined.
 * @param {unknown} extensions
 * @returns {string | undefined}
 */
function unitOf(extensions) {
    if (extensions === null || typeof extensions !== 'object') return undefined;
    const own = /** @type {Record<string, unknown>} */ (extensions)[EXTENSION];
    if (own === null || typeof own !== 'object') return undefined;
    const { unit } = /** @type {Record<string, unknown>} */ (own);
    return unit === undefined ? undefined : String(unit);
}

/**
 * Why `value` is not an srgb colour whose hex matches its components, or ''.
 * @param {unknown} value
 */
function notColour(value) {
    if (value === null || typeof value !== 'object') return 'it is not an object';
    const { colorSpace, components, alpha, hex } = /** @type {Record<string, unknown>} */ (value);
    if (colorSpace !== 'srgb') return 'its colorSpace is not srgb';
    if (typeof hex !== 'string' || !/^#[0-9a-fA-F]{6}$/.test(hex)) return 'it has no 6 digit hex';
    if (alpha !== undefined && alpha !== 1) return 'it has an alpha, which no output can write';
    const parts = Array.isArray(components) ? components : [];
    if (parts.length !== 3 || !parts.every((c) => typeof c === 'number' && c >= 0 && c <= 1))
        return 'it needs three components from 0 to 1';
    const bytes = [1, 3, 5].map((at) => parseInt(hex.slice(at, at + 2), 16));
    if (parts.some((c, at) => Math.round(c * 255) !== bytes[at]))
        return 'its components are not its hex';
    return '';
}

/**
 * Why `value` is not a dimension in px or rem, or ''.
 * @param {unknown} value
 */
function notDimension(value) {
    if (typeof value === 'number') return 'it has no unit';
    if (value === null || typeof value !== 'object')
        return 'it is not an object with a value and a unit';
    const { value: amount, unit } = /** @type {Record<string, unknown>} */ (value);
    if (typeof amount !== 'number' || !Number.isFinite(amount)) return 'its value is not a number';
    if (unit === undefined) return 'it has no unit';
    if (typeof unit !== 'string' || !UNITS.includes(unit)) return 'its unit is not px or rem';
    return '';
}

/**
 * Fails when the extension of Tumult gives a unit other than % or gives
 * it to a token that is not a number.
 * @type {(path: string, type: string, unit: string | undefined) => void}
 */
function checkUnit(path, type, unit) {
    if (unit === undefined) return;
    if (type !== 'number') fail(path, `only a number takes a unit from the extension ${EXTENSION}`);
    if (unit !== PERCENT)
        fail(path, `the extension ${EXTENSION} gives a unit other than ${PERCENT}`);
}

/** @type {(path: string, type: string, value: unknown) => void} */
function check(path, type, value) {
    if (type === 'color') {
        const why = notColour(value);
        if (why) fail(path, `$type is color, but the value is not a colour: ${why}`);
        return;
    }
    if (type === 'dimension') {
        const why = notDimension(value);
        if (why) fail(path, `$type is dimension, but the value is not a dimension: ${why}`);
        return;
    }
    if (type === 'fontWeight') {
        if (typeof value !== 'number' || !(value >= 1 && value <= 1000))
            fail(path, '$type is fontWeight, but the value is not a number from 1 to 1000');
        return;
    }
    if (type === 'number') {
        if (typeof value !== 'number' || !Number.isFinite(value))
            fail(path, '$type is number, but the value is not a number');
        return;
    }
    if (type !== 'fontFamily') fail(path, `build-tokens cannot write $type ${type}`);
    if (![value].flat().every((name) => typeof name === 'string' && name))
        fail(path, '$type is fontFamily, but the value is not a font name or a list of them');
}

/**
 * Every token of `sources`, merged in order (the last token of a path wins),
 * with its aliases resolved and its value checked against its type.
 * @param {Source[]} sources
 * @returns {Map<string, Token>}
 */
export function resolveTokens(sources) {
    /** @type {Map<string, Token>} */
    const raw = new Map();
    for (const { set, tokens } of sources) flatten(tokens, set, raw);
    /** @type {Map<string, Token>} */
    const done = new Map();
    /** @type {(path: string, trail: string[]) => Token} */
    const resolve = (path, trail) => {
        const known = done.get(path);
        if (known) return known;
        const token = /** @type {Token} */ (raw.get(path));
        const alias = typeof token.value === 'string' ? ALIAS.exec(token.value)?.[1] : undefined;
        let { value, type, unit } = token;
        if (alias !== undefined) {
            const at = [...trail, path];
            const loop = [...at.slice(at.indexOf(alias)), alias].join(' -> ');
            if (at.includes(alias)) fail(alias, `the aliases go round in a loop: ${loop}`);
            if (!raw.has(alias)) fail(path, `the alias {${alias}} names no token`);
            const target = resolve(alias, at);
            value = target.value;
            type ??= target.type;
            unit ??= target.unit;
        }
        if (type === undefined)
            fail(path, 'no $type on the token, on a group above it or on the token it names');
        check(path, type, value);
        checkUnit(path, type, unit);
        const resolved = { value, type, set: token.set, ...(unit === undefined ? {} : { unit }) };
        done.set(path, resolved);
        return resolved;
    };
    return new Map([...raw.keys()].map((path) => [path, resolve(path, [])]));
}

/** @param {string} file */
function readJson(file) {
    try {
        return JSON.parse(readFileSync(file, 'utf8'));
    } catch (error) {
        throw new Error(`${basename(file)}: ${message(error)}`, { cause: error });
    }
}

/**
 * The resolved tokens of each context of the theme modifier, and its default.
 * @param {string} dir
 */
function readThemes(dir) {
    const resolver = /** @type {Resolver} */ (readJson(join(dir, 'tumult.resolver.json')));
    if (resolver.version !== '2025.10') fail('tumult.resolver.json', 'its version is not 2025.10');
    const theme = resolver.modifiers.theme;
    if (!theme.contexts[theme.default]) fail('tumult.resolver.json', 'the theme has no default');
    /** @type {(refs: Ref[], set: string) => Source[]} */
    const load = (refs, set) =>
        refs.map(({ $ref }) => ({ set, tokens: readJson(join(dir, $ref)) }));
    /** @type {(context: string) => Source[]} */
    const sources = (context) =>
        resolver.resolutionOrder.flatMap(({ $ref }) => {
            if ($ref === '#/modifiers/theme') return load(theme.contexts[context], 'theme');
            const set = /^#\/sets\/(.+)$/.exec($ref)?.[1] ?? '';
            if (!resolver.sets[set]) fail('tumult.resolver.json', `cannot read ${$ref}`);
            return load(resolver.sets[set].sources, set);
        });
    /** @type {Record<string, Map<string, Token>>} */
    const themes = {};
    for (const context of Object.keys(theme.contexts)) {
        try {
            themes[context] = resolveTokens(sources(context));
        } catch (error) {
            throw new Error(`${context} theme, ${message(error)}`, { cause: error });
        }
    }
    return { fallback: theme.default, themes };
}

/**
 * A value as CSS writes it: a hex, a length, a number, a number with the
 * unit of the extension of Tumult or a list of font names. A font name of
 * more than one word, such as Segoe UI, is quoted. One word may start with
 * a dash, as -apple-system does.
 * @param {Token} token
 */
function cssValue(token) {
    if (token.type === 'color') return /** @type {{ hex: string }} */ (token.value).hex;
    if (token.type === 'dimension') {
        const { value, unit } = /** @type {{ value: number, unit: string }} */ (token.value);
        return `${value}${unit}`;
    }
    if (token.type === 'number') return `${token.value}${token.unit ?? ''}`;
    if (token.type === 'fontWeight') return String(token.value);
    const names = /** @type {string[]} */ ([token.value].flat());
    return names.map((name) => (/^-?[A-Za-z][\w-]*$/.test(name) ? name : `'${name}'`)).join(', ');
}

/**
 * The custom properties of the tokens a page may use: name to value.
 * @param {Map<string, Token>} tokens
 */
function properties(tokens) {
    return new Map(
        [...tokens]
            .filter(
                ([path, token]) =>
                    token.set !== PRIMITIVE &&
                    token.set !== DOCS_ONLY &&
                    !path.startsWith(CANVAS_ONLY),
            )
            .map(([path, token]) => [`--${path.replaceAll('.', '-')}`, cssValue(token)]),
    );
}

/** @type {(selector: string, scheme: string, props: Map<string, string>, indent: string) => string[]} */
const block = (selector, scheme, props, indent) => [
    `${indent}${selector} {`,
    `${indent}    color-scheme: ${scheme};`,
    ...[...props].map(([name, value]) => `${indent}    ${name}: ${value};`),
    `${indent}}`,
];

/**
 * tokens.css: the default theme at :root, and each other theme for its
 * `data-theme`, and for `data-theme="auto"` when the system asks for it.
 * @param {string} fallback
 * @param {Record<string, Map<string, Token>>} themes
 */
function css(fallback, themes) {
    const base = properties(themes[fallback]);
    const lines = [
        '/* Written by scripts/build-tokens.mjs from web/tokens/. Do not edit:',
        '   change a token file and run `npm run build:tokens`. */',
        '',
        `/* The ${fallback} theme is the default. color-scheme makes native widgets,`,
        '   such as popups and scroll bars, follow the theme. */',
        ...block(':root', fallback, base, ''),
    ];
    for (const [name, tokens] of Object.entries(themes)) {
        if (name === fallback) continue;
        const own = new Map([...properties(tokens)].filter(([prop, v]) => base.get(prop) !== v));
        lines.push(
            '',
            `/* data-theme="${name}" on <html> asks for the ${name} theme, and`,
            '   data-theme="auto" follows the system. */',
            ...block(`:root[data-theme='${name}']`, name, own, ''),
            `@media (prefers-color-scheme: ${name}) {`,
            ...block(":root[data-theme='auto']", name, own, '    '),
            '}',
        );
    }
    return `${lines.join('\n')}\n`;
}

/**
 * The colour roles of a theme as custom properties: name to value.
 * @param {Map<string, Token>} tokens
 */
const colours = (tokens) =>
    properties(new Map([...tokens].filter(([, token]) => token.type === 'color')));

/** @type {(props: Map<string, string>, indent: string) => string[]} */
const declarations = (props, indent) =>
    [...props].map(([name, value]) => `${indent}${name}: ${value};`);

/**
 * The colours of the CLI report. A report is a file that a reader opens on
 * its own, so it is light, and dark when the system asks for it, as it was
 * before the tokens. Its type and spacing are its own.
 * @param {Record<string, Map<string, Token>>} themes
 */
function reportCss(themes) {
    const light = colours(themes.light);
    return [
        '/* Written by web/scripts/build-tokens.mjs from web/tokens/. Do not edit:',
        '   change a token file and run `npm run build:tokens` in web/. */',
        '',
        '/* The colour roles of Krönika. color-scheme makes native widgets, such',
        '   as scroll bars, follow the light or dark setting of the system. */',
        ':root {',
        '    color-scheme: light dark;',
        ...declarations(light, '    '),
        '}',
        '@media (prefers-color-scheme: dark) {',
        '    :root {',
        ...declarations(colours(themes.dark), '        '),
        '    }',
        '}',
        '',
        '/* In print the page is white, as paper is. */',
        '@media print {',
        '    :root {',
        `        --bg: ${light.get('--bg-panel')};`,
        '    }',
        '}',
        '',
    ].join('\n');
}

/**
 * The colours of the group `docs.<group>` of a theme: name in the group to hex.
 * @param {Map<string, Token>} tokens
 * @param {string} group
 */
function docsColours(tokens, group) {
    const prefix = `docs.${group}.`;
    return [...tokens]
        .filter(([path]) => path.startsWith(prefix))
        .map(([path, token]) => {
            if (token.type !== 'color') fail(path, 'a token of the docs set must be a colour');
            return [path.slice(prefix.length), /** @type {{ hex: string }} */ (token.value).hex];
        });
}

/**
 * The colour variables of the docs theme. The site is light only. GitHub
 * Pages compiles them with Ruby Sass 3, so they are plain variables.
 * @param {Record<string, Map<string, Token>>} themes
 */
function docsScheme(themes) {
    return [
        '// Written by web/scripts/build-tokens.mjs from web/tokens/. Do not edit:',
        '// change a token file and run `npm run build:tokens` in web/.',
        '//',
        '// The colour variables of the docs theme, from the light theme of',
        '// Krönika. tumult.scss imports them.',
        '',
        ...docsColours(themes.light, 'scheme').map(([name, hex]) => `$${name}: ${hex};`),
        '',
    ].join('\n');
}

/**
 * The colours of the landing page: light, and a terminal that is dark on
 * purpose, in the dark theme.
 * @param {Record<string, Map<string, Token>>} themes
 */
function landingCss(themes) {
    return [
        '/* Written by web/scripts/build-tokens.mjs from web/tokens/. Do not edit:',
        '   change a token file and run `npm run build:tokens` in web/. */',
        '',
        '/* The colours of the landing page, docs/index.html: the light theme of',
        '   Krönika and the copper of the logo. The GameDay terminal (--term-)',
        '   is dark on purpose, like the terminal player above it. */',
        ':root {',
        ...docsColours(themes.light, 'landing').map(([name, hex]) => `    --${name}: ${hex};`),
        ...docsColours(themes.dark, 'terminal').map(([name, hex]) => `    --term-${name}: ${hex};`),
        '}',
        '',
    ].join('\n');
}

/** @type {(width: number, height: number, title: string, body: string[]) => string} */
const svg = (width, height, title, body) =>
    [
        `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}" viewBox="0 0 ${width} ${height}" role="img">`,
        `<title>${title}</title>`,
        ...body,
        '</svg>',
        '',
    ].join('\n');

/**
 * A swatch: a box of each colour side by side, with a thin edge, so that a
 * white swatch shows on the white page of the docs.
 * @param {string} title
 * @param {string[]} hexes
 * @param {string} edge
 */
function swatch(title, hexes, edge) {
    const cell = hexes.length === 1 ? 48 : 24;
    const width = cell * hexes.length;
    return svg(width, 24, title, [
        ...hexes.map(
            (hex, at) => `<rect x="${at * cell}" y="0" width="${cell}" height="24" fill="${hex}"/>`,
        ),
        `<rect x="0.5" y="0.5" width="${width - 1}" height="23" fill="none" stroke="${edge}"/>`,
    ]);
}

/** @type {(token: Token) => string} */
const hexOf = (token) => /** @type {{ hex: string }} */ (token.value).hex;

/**
 * The colour roles of a theme that pages use, by name: the colours at the
 * top level of the tokens, as tokens.gen.ts has them.
 * @param {Map<string, Token>} tokens
 */
const roles = (tokens) =>
    [...tokens].filter(
        ([path, token]) =>
            !path.includes('.') &&
            token.type === 'color' &&
            ![PRIMITIVE, PAGE_ONLY, DOCS_ONLY].includes(token.set),
    );

/**
 * The number and unit of a step of the scale: a dimension, or a number with
 * the unit of the extension of Tumult, as 50%.
 * @param {Token} token
 * @returns {{ value: number, unit: string }}
 */
function stepOf(token) {
    if (typeof token.value === 'number') return { value: token.value, unit: token.unit ?? '' };
    return /** @type {{ value: number, unit: string }} */ (token.value);
}

/**
 * One group of the scale from `top`, at x: its title, then a row for each of
 * its steps with the name and the value, and the step drawn at its size.
 * The lines of the picture and the y below the group.
 * @param {Map<string, Token>} light
 * @param {Group} group
 * @param {number} x
 * @param {number} top
 * @param {Look & { label: string }} look
 */
function scaleGroup(light, group, x, top, look) {
    const lines = [
        `<text x="${x}" y="${top + 24}" font-family="${look.font}" font-size="15" font-weight="600" fill="${look.text}">${group.title}</text>`,
    ];
    let y = top + 36;
    for (const [path, token] of light) {
        if (!path.startsWith(group.prefix)) continue;
        const { value, unit } = stepOf(token);
        // A percent is of the 35 px box that the radius group draws.
        const px = unit === 'rem' ? value * 16 : unit === PERCENT ? (value / 100) * 35 : value;
        const row = group.row(px);
        const base = y + Math.round(row / 2) + 4;
        lines.push(`<text x="${x}" y="${base}" ${look.label}>${path} ${value}${unit}</text>`);
        lines.push(group.draw(x, y, base, px, look));
        y += row;
    }
    return { lines, bottom: y };
}

/**
 * The type, spacing and radius scale of the pages, in the light theme: the
 * type across the top, and spacing and radius side by side below it.
 * @param {Map<string, Token>} light
 */
function scale(light) {
    const css = properties(light);
    /** @type {(name: string) => string} */
    const prop = (name) => /** @type {string} */ (css.get(name));
    const look = {
        text: prop('--text'),
        fill: prop('--accent-dim'),
        edge: prop('--border-input'),
        font: prop('--font-body'),
        label: `font-family="${prop('--mono')}" font-size="12" fill="${prop('--text-dim')}"`,
    };
    const [type, spacing, radius] = SCALE;
    const top = scaleGroup(light, type, 24, 16, look);
    const left = scaleGroup(light, spacing, 24, top.bottom + 8, look);
    const right = scaleGroup(light, radius, 344, top.bottom + 8, look);
    const height = Math.max(left.bottom, right.bottom) + 24;
    return svg(640, height, 'The type, spacing and radius scale of Krönika', [
        `<rect x="0" y="0" width="640" height="${height}" fill="${prop('--bg-panel')}"/>`,
        ...top.lines,
        ...left.lines,
        ...right.lines,
    ]);
}

/**
 * The pictures of the tokens on the UI standard page of the docs: a swatch
 * of each colour role in each theme, a strip of the series of the charts of
 * each theme, and the scale. Path under web/ to its text.
 * @param {Record<string, Map<string, Token>>} themes
 */
function pictures(themes) {
    const edge = /** @type {string} */ (colours(themes.light).get('--border-input'));
    /** @type {Record<string, string>} */
    const out = {};
    for (const [theme, tokens] of Object.entries(themes)) {
        for (const [role, token] of roles(tokens))
            out[`${PICTURES}/swatch-${theme}-${role}.svg`] = swatch(
                `${role}, ${theme}: ${hexOf(token)}`,
                [hexOf(token)],
                edge,
            );
        const series = [...tokens]
            .filter(([path]) => /^chart\.series\.\d+$/.test(path))
            .sort(([a], [b]) => Number(a.split('.')[2]) - Number(b.split('.')[2]))
            .map(([, token]) => hexOf(token));
        out[`${PICTURES}/swatch-${theme}-chart-series.svg`] = swatch(
            `chart.series, ${theme}: ${series.join(', ')}`,
            series,
            edge,
        );
    }
    out[`${PICTURES}/scale.svg`] = scale(themes.light);
    return out;
}

/**
 * A value as the TS writes it: a px dimension, a font weight and a number
 * as a number, as ECharts takes them, and anything else as CSS writes it.
 * @param {Token} token
 */
function tsValue(token) {
    if (token.type === 'fontWeight' || token.type === 'number') return token.value;
    if (token.type !== 'dimension') return cssValue(token);
    const { value, unit } = /** @type {{ value: number, unit: string }} */ (token.value);
    return unit === 'px' ? value : cssValue(token);
}

/**
 * The tokens a page or a chart may use, as nested objects. A group of tokens
 * named 1, 2, 3 and on is a list.
 * @param {Map<string, Token>} tokens
 */
function nest(tokens) {
    /** @type {Record<string, any>} */
    const tree = {};
    for (const [path, token] of tokens) {
        if (token.set === PRIMITIVE || token.set === PAGE_ONLY || token.set === DOCS_ONLY) continue;
        const parts = path.split('.');
        const leaf = /** @type {string} */ (parts.pop());
        parts.reduce((at, part) => (at[part] ??= {}), tree)[leaf] = tsValue(token);
    }
    /** @type {(node: any) => any} */
    const lists = (node) => {
        if (typeof node !== 'object') return node;
        const keys = Object.keys(node);
        if (keys.every((key, at) => key === String(at + 1))) return keys.map((key) => node[key]);
        return Object.fromEntries(keys.map((key) => [key, lists(node[key])]));
    };
    return lists(tree);
}

/** @param {Record<string, Map<string, Token>>} themes */
function ts(themes) {
    const tree = Object.fromEntries(Object.entries(themes).map(([name, t]) => [name, nest(t)]));
    return [
        '// The design tokens of each theme, resolved, for code that cannot read a',
        '// CSS custom property: ECharts draws on a canvas. Written by',
        '// scripts/build-tokens.mjs from web/tokens/. Do not edit: change a token',
        '// file and run `npm run build:tokens`.',
        '',
        `export const tokens = ${JSON.stringify(tree, null, 4)} as const;`,
        '',
    ].join('\n');
}

/**
 * The outputs of the tokens in `dir`: a path under web/ to its text.
 * @param {string} dir
 * @returns {Record<string, string>}
 */
export function build(dir) {
    const { fallback, themes } = readThemes(dir);
    return {
        'src/lib/tokens.css': css(fallback, themes),
        'src/lib/tokens.gen.ts': ts(themes),
        [REPORT_CSS]: reportCss(themes),
        [DOCS_SCHEME]: docsScheme(themes),
        [LANDING_CSS]: landingCss(themes),
        ...pictures(themes),
    };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
    try {
        const outputs = build(join(WEB, 'tokens'));
        for (const [file, text] of Object.entries(outputs)) {
            mkdirSync(dirname(join(WEB, file)), { recursive: true });
            writeFileSync(join(WEB, file), text);
        }
    } catch (error) {
        console.error(`build-tokens: ${message(error)}`);
        process.exitCode = 1;
    }
}
