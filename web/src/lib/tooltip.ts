// The one place that writes tooltip markup. ECharts sets the string a
// tooltip `formatter` returns as HTML, and what a tooltip shows (names,
// labels, ids, reasons) comes from telemetry and from files, not from
// this code. So no page writes that string: a page hands lines of text
// to `tip`, which escapes every value, and gives ECharts the formatter
// `tipFormatter` makes of it. `tooltip.test.ts` fails when a tooltip
// formatter anywhere under `src` is anything else.

import { NEUTRAL_DOT } from './chart-theme';

const HTML_ESCAPES: Record<string, string> = {
    '&': '&amp;',
    '<': '&lt;',
    '>': '&gt;',
    '"': '&quot;',
    "'": '&#39;',
};

/** Text as HTML shows it: nothing in `value` is read as markup. */
export function escapeHtml(value: unknown): string {
    return String(value ?? '').replace(/[&<>"']/g, (c) => HTML_ESCAPES[c]);
}

/** Text to print in bold. Made by [`strong`]. */
class Strong {
    constructor(readonly text: unknown) {}
}

/**
 * Tooltip markup. Only [`tip`] makes one: the class is not exported, and
 * the private field keeps an object that merely has an `html` from
 * passing for one where the types are checked.
 */
class Tip {
    readonly #made = true;

    constructor(readonly html: string) {}

    /** Whether `value` was made by [`tip`]. */
    static is(value: unknown): value is Tip {
        return value instanceof Tip && value.#made;
    }
}

export type { Strong, Swatch, Tip };

/** A piece of a line: text, or text in bold. */
export type TipPart = Strong | Swatch | string | number | null | undefined;
/** A line of a tooltip: one piece or several; an empty one is left out. */
export type TipLine = TipPart | TipPart[];

/** The dot of a series in its colour. Made by [`swatch`]. */
class Swatch {
    constructor(readonly color: unknown) {}
}

/**
 * The dot ECharts puts before a series name, in `color`. A colour that
 * is not a plain CSS colour (a gradient object, or text with anything
 * else in it) gives a dot in [`NEUTRAL_DOT`], so every row keeps one.
 */
export function swatch(color: unknown): Swatch {
    return new Swatch(color);
}

/** The colour of a dot whose own colour cannot be written as one. */
export { NEUTRAL_DOT };

// A hex colour, an rgb()/hsl() call of numbers, or a colour name.
const PLAIN_COLOR = /^(#[0-9a-f]{3,8}|(rgb|hsl)a?\([\d\s.,%/]+\)|[a-z]+)$/i;

function swatchHtml(dot: Swatch): string {
    const plain = typeof dot.color === 'string' && PLAIN_COLOR.test(dot.color);
    const color = plain ? escapeHtml(dot.color) : NEUTRAL_DOT;
    const style = 'display:inline-block;margin-right:4px;border-radius:50%;width:10px;height:10px;';
    return `<span style="${style}background-color:${color}"></span>`;
}

/** `text` in bold. */
export function strong(text: unknown): Strong {
    return new Strong(text);
}

function partHtml(part: TipPart): string {
    if (part instanceof Strong) {
        const text = escapeHtml(part.text);
        return text === '' ? '' : `<b>${text}</b>`;
    }
    if (part instanceof Swatch) return swatchHtml(part);
    // Anything else is text, a tip or an object included.
    return escapeHtml(typeof part === 'object' && part !== null ? JSON.stringify(part) : part);
}

function lineHtml(line: TipLine): string {
    return Array.isArray(line) ? line.map(partHtml).join('') : partHtml(line);
}

/** A tooltip of these lines, every value in them escaped. */
export function tip(...lines: TipLine[]): Tip {
    const html = lines.map(lineHtml).filter((line) => line !== '');
    return new Tip(html.join('<br/>'));
}

/**
 * The `formatter` of an ECharts tooltip. `build` answers with a [`tip`]
 * or with nothing, which shows no tooltip.
 */
export function tipFormatter<P>(
    build: (params: P) => Tip | null | undefined,
): (params: P) => string {
    return (params) => {
        const built: unknown = build(params);
        if (built === null || built === undefined) return '';
        // The types allow nothing but a tip here; a cast could still lie.
        return Tip.is(built) ? built.html : lineHtml(built as TipPart);
    };
}
