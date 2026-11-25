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
// The lint fails on a raw value that the allow list does not hold, and on
// a line of the allow list that matches no raw value, so the list only
// shrinks. A line of the list is `path kind value`, with no line number,
// so an edit above a value does not break it. A value used twice in a
// file is on two lines.
//
// Usage: node scripts/style-literals.mjs (the lint)
//        node scripts/style-literals.mjs --list (prints each raw value by kind)

import { readFileSync, readdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const WEB = join(dirname(fileURLToPath(import.meta.url)), '..');
// The CSS that the token generator writes: its values are the tokens.
const TOKENS_CSS = 'src/lib/tokens.css';
// The raw values that the styles keep today, in web/.
const ALLOW_LIST = 'style-literals-allow.txt';

/** How to fix a failure of the lint, in one line. */
export const FIX = `Use a var() of a token in ${TOKENS_CSS} for a raw value, and delete each line of ${ALLOW_LIST} that matches nothing.`;

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

/**
 * The names of the tokens of each kind in tokens.css.
 * @type {Record<string, RegExp>}
 */
const TOKEN_NAMES = {
    'font-size': /^font-size-/,
    'font-weight': /^font-weight-/,
    'line-height': /^line-height-/,
    'letter-spacing': /^letter-spacing-/,
    'font-family': /^(?:mono$|font-(?!size-|weight-))/,
    spacing: /^space-/,
    radius: /^radius-/,
    'border-width': /^border-width-/,
    'z-index': /^z-/,
};

/** @typedef {{ kind: string, value: string, property: string, file: string, line: number }} Literal */
/** @typedef {{ path: string, kind: string, value: string, line: number }} Allowed */

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

/**
 * A value as the lint compares it with a token: in lower case, with one
 * space for each run of white space.
 * @param {string} value
 */
function normal(value) {
    return value.trim().toLowerCase().replace(/\s+/g, ' ');
}

/**
 * The tokens of tokens.css that have one value in every theme, as a map
 * from `kind value` to the name of the token.
 * @param {string} css
 * @returns {Map<string, string>}
 */
function tokensByValue(css) {
    /** @type {Map<string, Set<string>>} */
    const values = new Map();
    for (const [, name, value] of css.matchAll(/--([\w-]+)\s*:\s*([^;]+);/g))
        values.set(name, (values.get(name) ?? new Set()).add(normal(value)));
    /** @type {Map<string, string>} */
    const byValue = new Map();
    for (const [name, set] of values) {
        if (set.size !== 1) continue;
        const [value] = set;
        for (const kind of KINDS) {
            const key = `${kind} ${value}`;
            if (TOKEN_NAMES[kind].test(name) && !byValue.has(key)) byValue.set(key, name);
        }
    }
    return byValue;
}

/**
 * The lines of an allow list, with their line numbers, and a problem for
 * each line that is not `path kind value`. A line that starts with # is a
 * comment.
 * @param {string} text
 * @returns {{ allowed: Allowed[], problems: string[] }}
 */
function readAllowList(text) {
    /** @type {Allowed[]} */
    const allowed = [];
    /** @type {string[]} */
    const problems = [];
    for (const [at, raw] of text.split(/\r?\n/).entries()) {
        const line = raw.trim();
        if (line === '' || line.startsWith('#')) continue;
        const entry = /^(\S+)\s+(\S+)\s+(.+)$/.exec(line);
        if (!entry || !KINDS.includes(entry[2])) {
            problems.push(`${ALLOW_LIST}:${at + 1}: "${line}" is not a path, a kind and a value`);
            continue;
        }
        allowed.push({ path: entry[1], kind: entry[2], value: entry[3], line: at + 1 });
    }
    return { allowed, problems };
}

/** @type {(path: string, kind: string, value: string) => string} */
const keyOf = (path, kind, value) => JSON.stringify([path, kind, value]);

/**
 * The problems of the uses of one raw value that the allow list holds
 * `held` times: one per use when it holds none, else one for the file.
 * @param {Literal[]} uses
 * @param {number} held
 * @param {string | undefined} token
 * @returns {string[]}
 */
function notAllowed(uses, held, token) {
    const hint = token ? `; use var(--${token})` : '';
    const { file, kind, value } = uses[0];
    if (held === 0)
        return uses.map(
            ({ line }) => `${file}:${line}: raw ${kind} ${value} is not on the allow list${hint}`,
        );
    const lines = uses.map(({ line }) => line).sort((a, b) => a - b);
    const where = `${file}:${lines.at(-1)}: raw ${kind} ${value}`;
    return [
        `${where} is used ${uses.length} times and the allow list holds ${held} (lines ${lines.join(', ')})${hint}`,
    ];
}

/**
 * The problems of the raw values `literals`, given the text of the allow
 * list and of tokens.css, one line each: a raw value used more often than
 * the list holds it, with the token that has its exact value, and a line
 * of the list that matches no raw value or is not `path kind value`.
 * @param {Literal[]} literals
 * @param {string} allowList
 * @param {string} tokensCss
 * @returns {string[]}
 */
export function lint(literals, allowList, tokensCss) {
    const { allowed, problems: badLines } = readAllowList(allowList);
    const tokens = tokensByValue(tokensCss);
    /** @type {Map<string, Literal[]>} */
    const uses = new Map();
    for (const literal of literals) {
        const key = keyOf(literal.file, literal.kind, literal.value);
        uses.set(key, [...(uses.get(key) ?? []), literal]);
    }
    /** @type {Map<string, number>} */
    const held = new Map();
    for (const { path, kind, value } of allowed) {
        const key = keyOf(path, kind, value);
        held.set(key, (held.get(key) ?? 0) + 1);
    }
    const unlisted = [...uses].flatMap(([key, mine]) => {
        const count = held.get(key) ?? 0;
        if (mine.length <= count) return [];
        return notAllowed(mine, count, tokens.get(`${mine[0].kind} ${normal(mine[0].value)}`));
    });
    return [...unlisted, ...badLines, ...stale(allowed, uses)];
}

/**
 * A problem for each line of the allow list beyond the uses of its value.
 * @param {Allowed[]} allowed
 * @param {Map<string, Literal[]>} uses
 * @returns {string[]}
 */
function stale(allowed, uses) {
    /** @type {Map<string, number>} */
    const left = new Map([...uses].map(([key, mine]) => [key, mine.length]));
    return allowed.flatMap(({ path, kind, value, line }) => {
        const key = keyOf(path, kind, value);
        const count = left.get(key) ?? 0;
        left.set(key, count - 1);
        if (count > 0) return [];
        return [
            `${ALLOW_LIST}:${line}: ${path} ${kind} ${value} matches no raw value; delete the line`,
        ];
    });
}

/**
 * The problems of the styles of src/ under `web`, with its allow list and
 * tokens.css, as `lint` finds them.
 * @param {string} web
 * @returns {string[]}
 */
export function lintStyles(web) {
    /** @type {(path: string) => string} */
    const read = (path) => readFileSync(join(web, path), 'utf8');
    return lint(scan(web), read(ALLOW_LIST), read(TOKENS_CSS));
}

/**
 * Prints the problems of the lint and returns the exit code.
 * @param {string[]} problems
 */
function report(problems) {
    if (problems.length === 0) {
        console.log(`The styles keep only the raw values of ${ALLOW_LIST}.`);
        return 0;
    }
    for (const problem of problems) console.error(problem);
    console.error(`${problems.length} style lint failure(s). ${FIX}`);
    return 1;
}

/**
 * Prints each raw value of `found`, by kind.
 * @param {Literal[]} found
 */
function list(found) {
    for (const kind of KINDS) {
        const mine = found.filter((literal) => literal.kind === kind);
        console.log(`${kind}: ${mine.length}`);
        for (const { file, line, property, value } of mine)
            console.log(`    ${file}:${line} ${property} ${value}`);
    }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
    if (process.argv.includes('--list')) list(scan(WEB));
    else process.exitCode = report(lintStyles(WEB));
}
