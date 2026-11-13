// The one place that writes tooltip markup. ECharts sets the string a
// tooltip `formatter` returns as HTML, and what a tooltip shows (names,
// labels, ids, reasons) comes from telemetry and from files, not from
// this code. So no page writes that string: a page hands lines of text
// to `tip`, which escapes every value, and gives ECharts the formatter
// `tipFormatter` makes of it. `tooltip.test.ts` fails when a tooltip
// formatter anywhere under `src` is anything else.

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

export type { Strong, Tip };

/** A piece of a line: text, or text in bold. */
export type TipPart = Strong | string | number | null | undefined;
/** A line of a tooltip: one piece or several; an empty one is left out. */
export type TipLine = TipPart | TipPart[];

/** `text` in bold. */
export function strong(text: unknown): Strong {
    return new Strong(text);
}

function partHtml(part: TipPart): string {
    if (part instanceof Strong) {
        const text = escapeHtml(part.text);
        return text === '' ? '' : `<b>${text}</b>`;
    }
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
