#!/usr/bin/env node
// Find the raw CSS values of the kinds that have design tokens: font sizes,
// font weights, line heights, letter spacing, font families, spacing,
// radii, border widths and z-index. It reads the <style> blocks and the
// style attributes of the Svelte files under src/, and the CSS files there
// but tokens.css, which the token generator writes. A value inside var()
// is not raw, and neither is 0, a keyword or a value that a script works
// out. This is a text scan, not a CSS parser: a `;` or a brace in a string
// of a style would confuse it, and Krönika has none.
//
// Usage: node scripts/style-literals.mjs (prints each raw value by kind)

import { readFileSync, readdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const WEB = join(dirname(fileURLToPath(import.meta.url)), '..');
// The CSS that the token generator writes: its values are the tokens.
const TOKENS_CSS = 'src/lib/tokens.css';

/** The kinds of value that have tokens, in the order they are printed. */
export const KINDS = [
    'font-size',
    'font-weight',
    'line-height',
    'letter-spacing',
    'font-family',
    'spacing',
    'radius',
    'border-width',
    'z-index',
];

/** @typedef {{ kind: string, value: string, property: string, file: string, line: number }} Literal */

// A number and its unit, not part of a word, a hex colour or a longer number.
const NUMBER = /(?<![\w#.-])-?(?:\d+\.?\d*|\.\d+)([a-z]+|%)?/gi;
// The size of a font shorthand, and the line height after its slash.
const FONT_SIZE =
    /(?<![\w#.-])((?:\d+\.?\d*|\.\d+)(?:[a-z]+|%))(?:\s*\/\s*((?:\d+\.?\d*|\.\d+)(?:[a-z]+|%)?))?/i;
const HEX = /#[0-9a-f]{3,8}\b/gi;
const WIDE = /^(?:inherit|initial|unset|revert|revert-layer)$/;

/** @type {(text: string) => string} */
const blank = (text) => text.replace(/[^\n]/g, ' ');

/**
 * The kind of value a property takes, or '' when it has no tokens.
 * @param {string} property
 */
function kindOf(property) {
    if (/^(?:margin|padding)(?:-[a-z-]+)?$/.test(property)) return 'spacing';
    if (/^(?:row-|column-)?gap$/.test(property)) return 'spacing';
    if (/^border(?:-[a-z]+)*-radius$/.test(property)) return 'radius';
    if (/^(?:border|outline)(?:-(?:top|right|bottom|left))?(?:-width)?$/.test(property))
        return 'border-width';
    return KINDS.includes(property) ? property : '';
}

/**
 * `text` with each `open ... close` part blanked out, nested ones too.
 * @param {string} text
 * @param {string} open
 * @param {string} close
 */
function without(text, open, close) {
    const inner = open.at(-1);
    let out = '';
    let depth = 0;
    for (let at = 0; at < text.length; at++) {
        if (depth === 0 && text.startsWith(open, at)) {
            depth = 1;
            out += blank(open);
            at += open.length - 1;
            continue;
        }
        if (depth === 0) {
            out += text[at];
            continue;
        }
        if (text[at] === inner) depth += 1;
        if (text[at] === close) depth -= 1;
        out += blank(text[at]);
    }
    return out;
}

/**
 * Whether a number with `unit` in a value of `kind` is raw: not 0, and
 * with a unit where the kind takes one.
 * @param {string} kind
 * @param {number} number
 * @param {string} unit
 */
function isRaw(kind, number, unit) {
    if (kind === 'z-index') return unit === '';
    if (number === 0) return false;
    if (kind === 'line-height') return true;
    if (kind === 'font-weight') return unit === '';
    if (kind === 'border-width') return ['px', 'em', 'rem'].includes(unit);
    return unit !== '';
}

/**
 * The raw values of a `font` shorthand: its size, the line height after
 * the slash and the family.
 * @param {string} text
 * @returns {[string, string][]}
 */
function fontShorthand(text) {
    if (WIDE.test(text)) return [];
    /** @type {[string, string][]} */
    const found = [];
    const size = FONT_SIZE.exec(text);
    if (size) found.push(['font-size', size[1]]);
    if (size?.[2]) found.push(['line-height', size[2]]);
    const family = size ? text.slice(size.index + size[0].length).trim() : text;
    if (/[a-z]/i.test(family)) found.push(['font-family', family]);
    return found;
}

/**
 * The raw values of one declaration, as [kind, value].
 * @param {string} property
 * @param {string} value
 * @returns {[string, string][]}
 */
function rawValues(property, value) {
    const text = without(value, 'var(', ')').replace(HEX, blank).trim();
    if (property === 'font') return fontShorthand(text);
    const kind = kindOf(property);
    if (!kind || WIDE.test(text)) return [];
    if (kind === 'font-family') return /[a-z]/i.test(text) ? [[kind, text]] : [];
    /** @type {[string, string][]} */
    const found = [];
    for (const [number, unit = ''] of text.matchAll(NUMBER)) {
        const amount = Number(number.slice(0, number.length - unit.length));
        if (isRaw(kind, amount, unit.toLowerCase())) found.push([kind, number]);
    }
    return found;
}

/**
 * The declarations of CSS text, as [property, value, offset of the
 * property]. Custom properties are left out.
 * @param {string} css
 * @returns {[string, string, number][]}
 */
function declarations(css) {
    const text = css.replace(/\/\*[\s\S]*?\*\//g, blank);
    /** @type {[string, string, number][]} */
    const found = [];
    let start = 0;
    for (let at = 0; at <= text.length; at++) {
        const end = at === text.length || ';{}'.includes(text[at]);
        if (!end) continue;
        const declaration = /^(\s*)([a-z][a-z-]*)\s*:([\s\S]*)$/i.exec(text.slice(start, at));
        if (declaration && text[at] !== '{')
            found.push([
                declaration[2].toLowerCase(),
                declaration[3].trim(),
                start + declaration[1].length,
            ]);
        start = at + 1;
    }
    return found;
}

/**
 * The parts of a file that hold CSS, as [text, offset in the file]. The
 * Svelte expressions of a style attribute are blanked out: a script works
 * their values out.
 * @param {string} file
 * @param {string} text
 * @returns {[string, number][]}
 */
function cssParts(file, text) {
    if (file.endsWith('.css')) return [[text, 0]];
    /** @type {[string, number][]} */
    const parts = [];
    for (const block of text.matchAll(/(<style[^>]*>)([\s\S]*?)<\/style>/g))
        parts.push([block[2], (block.index ?? 0) + block[1].length]);
    const markup = text.replace(/<(script|style)[^>]*>[\s\S]*?<\/\1>/g, blank);
    for (const attr of markup.matchAll(/(\sstyle=")([^"]*)"/g))
        parts.push([without(attr[2], '{', '}'), (attr.index ?? 0) + attr[1].length]);
    return parts;
}

/**
 * Every raw value in `text`, the content of `file`.
 * @param {string} file
 * @param {string} text
 * @returns {Literal[]}
 */
export function literalsIn(file, text) {
    /** @type {(offset: number) => number} */
    const lineAt = (offset) => text.slice(0, offset).split('\n').length;
    return cssParts(file, text).flatMap(([css, base]) =>
        declarations(css).flatMap(([property, value, offset]) =>
            rawValues(property, value).map(([kind, raw]) => ({
                kind,
                value: raw,
                property,
                file,
                line: lineAt(base + offset),
            })),
        ),
    );
}

/**
 * The Svelte and CSS files under `dir` of `web`, as paths from `web`.
 * @param {string} web
 * @param {string} dir
 * @returns {string[]}
 */
function styleFiles(web, dir) {
    return readdirSync(join(web, dir), { withFileTypes: true }).flatMap((entry) => {
        const path = `${dir}/${entry.name}`;
        if (entry.isDirectory()) return styleFiles(web, path);
        if (!/\.(?:svelte|css)$/.test(entry.name) || path === TOKENS_CSS) return [];
        return [path];
    });
}

/**
 * Every raw value in the styles of src/ under `web`, in file order.
 * @param {string} web
 * @returns {Literal[]}
 */
export function scan(web) {
    return styleFiles(web, 'src')
        .sort()
        .flatMap((file) => literalsIn(file, readFileSync(join(web, file), 'utf8')));
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
    const found = scan(WEB);
    for (const kind of KINDS) {
        const mine = found.filter((literal) => literal.kind === kind);
        console.log(`${kind}: ${mine.length}`);
        for (const { file, line, property, value } of mine)
            console.log(`    ${file}:${line} ${property} ${value}`);
    }
}
