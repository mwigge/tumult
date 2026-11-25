#!/usr/bin/env node
// Write the outputs of the design tokens of Krönika from web/tokens/, which
// hold DTCG 2025.10 JSON: tumult.resolver.json names the sets and the theme
// modifier. For each theme this merges the sets in order, resolves every
// alias and checks every value. It writes src/lib/tokens.css (the custom
// properties of each theme) and src/lib/tokens.gen.ts (the same values for
// ECharts, which cannot read CSS). Both are committed, and CI fails when
// they are not what this writes. It reads only the part of the format that
// Tumult uses: srgb colours with a hex, font families, dimensions in px or
// rem, font weights and numbers as numbers, aliases, sets and one modifier.
//
// Usage: node scripts/build-tokens.mjs

import { readFileSync, writeFileSync } from 'node:fs';
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
// Charts draw on a canvas and cannot read CSS, so this group is only in the TS.
const CANVAS_ONLY = 'chart.';

/** @typedef {{ value: unknown, type?: string, set: string }} Token */
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
        into.set(path.join('.'), { value: tree.$value, type: own, set });
        return;
    }
    for (const [name, child] of Object.entries(tree)) {
        if (name.startsWith('$') || child === null || typeof child !== 'object') continue;
        flatten(/** @type {Record<string, unknown>} */ (child), set, into, [...path, name], own);
    }
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
        let { value, type } = token;
        if (alias !== undefined) {
            const at = [...trail, path];
            const loop = [...at.slice(at.indexOf(alias)), alias].join(' -> ');
            if (at.includes(alias)) fail(alias, `the aliases go round in a loop: ${loop}`);
            if (!raw.has(alias)) fail(path, `the alias {${alias}} names no token`);
            const target = resolve(alias, at);
            value = target.value;
            type ??= target.type;
        }
        if (type === undefined)
            fail(path, 'no $type on the token, on a group above it or on the token it names');
        check(path, type, value);
        const resolved = { value, type, set: token.set };
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
 * A value as CSS writes it: a hex, a length, a number or a list of font
 * names. A font name of more than one word, such as Segoe UI, is quoted.
 * One word may start with a dash, as -apple-system does.
 * @param {Token} token
 */
function cssValue(token) {
    if (token.type === 'color') return /** @type {{ hex: string }} */ (token.value).hex;
    if (token.type === 'dimension') {
        const { value, unit } = /** @type {{ value: number, unit: string }} */ (token.value);
        return `${value}${unit}`;
    }
    if (token.type === 'fontWeight' || token.type === 'number') return String(token.value);
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
            .filter(([path, token]) => token.set !== PRIMITIVE && !path.startsWith(CANVAS_ONLY))
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
 * The tokens a page or a chart may use, as nested objects. A group of tokens
 * named 1, 2, 3 and on is a list.
 * @param {Map<string, Token>} tokens
 */
function nest(tokens) {
    /** @type {Record<string, any>} */
    const tree = {};
    for (const [path, token] of tokens) {
        if (token.set === PRIMITIVE || token.set === PAGE_ONLY) continue;
        const parts = path.split('.');
        const leaf = /** @type {string} */ (parts.pop());
        parts.reduce((at, part) => (at[part] ??= {}), tree)[leaf] = cssValue(token);
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
    return { 'src/lib/tokens.css': css(fallback, themes), 'src/lib/tokens.gen.ts': ts(themes) };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
    try {
        const outputs = build(join(WEB, 'tokens'));
        for (const [file, text] of Object.entries(outputs)) writeFileSync(join(WEB, file), text);
    } catch (error) {
        console.error(`build-tokens: ${message(error)}`);
        process.exitCode = 1;
    }
}
