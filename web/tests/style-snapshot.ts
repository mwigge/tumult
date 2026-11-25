// The style snapshot: what a user sees of each element of a page, read
// from the computed styles, and compared with a baseline committed under
// tests/style-snapshots/. Only values that do not depend on the fonts of
// the host are read, so the baseline of one host holds on another: no
// width, height or position, and the computed value (`auto`, `2%`), not
// the value the layout used. The one height read is that of a busy
// loading block, which the page sets.
import { existsSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

import { expect } from '@playwright/test';
import type { Page } from '@playwright/test';

export type Theme = 'dark' | 'light';

/** The styles of one element: property to value. A property at its default is left out. */
export type Styles = Record<string, string>;

/** The styles of every visible element of a page, by the key of the element. */
export type Elements = Record<string, Styles>;

export interface Snapshot {
    page: string;
    path: string;
    theme: Theme;
    width: number;
    height: number;
    elements: Elements;
}

/** One difference between a baseline and the page. */
export interface Change {
    key: string;
    property: string;
    before: string;
    after: string;
}

/** Set to 1 to write the baselines from the pages instead of comparing. */
export const UPDATE = process.env.STYLE_SNAPSHOT_UPDATE === '1';

export const BASELINES = fileURLToPath(new URL('./style-snapshots/', import.meta.url));

/** The properties an element takes from its parent. A child records one only where it differs. */
export const INHERITED = [
    'color',
    'font-family',
    'font-size',
    'font-weight',
    'line-height',
    'letter-spacing',
    'text-transform',
    'visibility',
];

/** An accessible name longer than this is cut in a key. */
const NAME_LENGTH = 40;

/** The value a left out property has: the default, or the value of the parent. */
const unset = (property: string) => (INHERITED.includes(property) ? '(as parent)' : '(default)');

/** At most this many changes are printed; the rest are counted. */
const SHOWN = 30;

export const fileOf = (page: string, theme: Theme, width: number) =>
    `${page}.${theme}.${width}.json`;

/** The `nodeType` of an element in the DOM tree the browser sends. */
const ELEMENT_NODE = 1;

/** The tag, role and accessible name of one element, in document order. */
export interface Label {
    tag: string;
    role: string;
    name: string;
}

/**
 * The tag, role and accessible name of every element, in document order,
 * from one read of the accessibility tree of the browser. An element the
 * tree ignores, or gives no role but `generic` or `none`, has no role.
 */
export async function labels(page: Page): Promise<Label[]> {
    const cdp = await page.context().newCDPSession(page);
    try {
        const { nodes } = await cdp.send('Accessibility.getFullAXTree');
        const { root } = await cdp.send('DOM.getDocument', { depth: -1 });
        const exposed = new Map<number, { role: string; name: string }>();
        for (const node of nodes) {
            if (node.ignored || node.backendDOMNodeId === undefined) continue;
            const role = String(node.role?.value ?? '');
            const name = String(node.name?.value ?? '')
                .replace(/\s+/g, ' ')
                .trim();
            exposed.set(node.backendDOMNodeId, {
                role: role === 'generic' || role === 'none' ? '' : role,
                name: name.length > NAME_LENGTH ? `${name.slice(0, NAME_LENGTH)}…` : name,
            });
        }
        const found: Label[] = [];
        const walk = (node: typeof root) => {
            if (node.nodeType === ELEMENT_NODE) {
                const label = exposed.get(node.backendNodeId);
                found.push({
                    tag: node.localName,
                    role: label?.role ?? '',
                    name: label?.name ?? '',
                });
            }
            for (const child of node.children ?? []) walk(child);
        };
        walk(root);
        return found;
    } finally {
        await cdp.detach();
    }
}

/**
 * Read the styles of every visible element of the page, or null when the
 * page changed while it was read. The key of an element is its tag, role,
 * classes and accessible name, with `#2`, `#3` for a later element of the
 * same key, after the keys of its ancestors that have a role, a class or
 * a name. A wrapper with none of them is not in the path, so it shows as
 * one element more, not as a change of every element in it. Class names
 * that Svelte derives from the CSS of a component are left out: they
 * change with any edit of that CSS.
 */
export async function readStyles(page: Page): Promise<Elements | null> {
    const found = await labels(page);
    return page.evaluate(
        ({ labels, inherited }) => {
            const HASH = /^svelte-[a-z0-9]+$/;
            const SIDES = ['top', 'right', 'bottom', 'left'];
            const CORNERS = ['top-left', 'top-right', 'bottom-right', 'bottom-left'];
            const elements: Record<string, Record<string, string>> = {};
            const taken = new Map<string, number>();
            let at = 0;

            const segment = (el: Element, role: string) => {
                const classes = [...el.classList].filter((c) => !HASH.test(c));
                return (
                    el.localName + (role ? `[${role}]` : '') + classes.map((c) => `.${c}`).join('')
                );
            };
            // Four values as the CSS shorthand writes them.
            const shorthand = ([top, right, bottom, left]: string[]) => {
                if (right !== left) return `${top} ${right} ${bottom} ${left}`;
                if (top !== bottom) return `${top} ${right} ${bottom}`;
                return top === right ? top : `${top} ${right}`;
            };

            const read = (el: Element, parent: Record<string, string> | null) => {
                const map = el.computedStyleMap();
                const get = (property: string) => map.get(property)?.toString() ?? '';
                const all: Record<string, string> = {};
                const out: Record<string, string> = {};
                const put = (property: string, value: string, unset: string) => {
                    if (value !== unset) out[property] = value;
                };
                for (const property of inherited) {
                    all[property] = get(property);
                    if (all[property] !== parent?.[property]) out[property] = all[property];
                }
                put('display', get('display'), '');
                put('grid-template-columns', get('grid-template-columns'), 'none');
                put('opacity', get('opacity'), '1');
                put('background-color', get('background-color'), 'rgba(0, 0, 0, 0)');
                put('background-image', get('background-image'), 'none');
                put('box-shadow', get('box-shadow'), 'none');
                put('z-index', get('z-index'), 'auto');
                // A loading block stands for the part that comes: its height is set.
                if (el.getAttribute('aria-busy') === 'true') put('height', get('height'), 'auto');
                for (const side of SIDES) {
                    const style = get(`border-${side}-style`);
                    if (style === 'none') continue;
                    const width = get(`border-${side}-width`);
                    out[`border-${side}`] = `${width} ${style} ${get(`border-${side}-color`)}`;
                }
                const radius = CORNERS.map((corner) => get(`border-${corner}-radius`));
                put('border-radius', shorthand(radius), '0px');
                put('padding', shorthand(SIDES.map((side) => get(`padding-${side}`))), '0px');
                put('margin', shorthand(SIDES.map((side) => get(`margin-${side}`))), '0px');
                const rowGap = get('row-gap');
                const columnGap = get('column-gap');
                put('gap', rowGap === columnGap ? rowGap : `${rowGap} ${columnGap}`, 'normal');
                const outline = get('outline-style');
                if (outline !== 'none') {
                    const offset = get('outline-offset');
                    out.outline =
                        `${get('outline-width')} ${outline} ${get('outline-color')}` +
                        (offset === '0px' ? '' : ` offset ${offset}`);
                }
                return { all, out };
            };

            // False when the element is not the next one the tree named:
            // the page changed between the two reads.
            const walk = (
                el: Element,
                path: string,
                parent: Record<string, string> | null,
            ): boolean => {
                const label = labels[at++];
                if (label?.tag !== el.localName) return false;
                const { all, out } = read(el, parent);
                const own = segment(el, label.role);
                const named = `${own}${label.name ? ` "${label.name}"` : ''}`;
                let key = `${path ? `${path} > ` : ''}${named}`;
                if (el.checkVisibility()) {
                    const count = (taken.get(key) ?? 0) + 1;
                    taken.set(key, count);
                    if (count > 1) key = `${key} #${count}`;
                    elements[key] = out;
                }
                const next = named === el.localName ? path : key;
                return [...el.children].every((child) => walk(child, next, all));
            };
            const whole = walk(document.documentElement, '', null) && at === labels.length;
            return whole ? elements : null;
        },
        { labels: found, inherited: INHERITED },
    );
}

/**
 * The styles of the page once they hold still: two reads a quarter of a
 * second apart give the same answer. Data, charts and the theme arrive
 * after the first paint.
 */
export async function settledStyles(page: Page): Promise<Elements> {
    let last = '';
    let styles: Elements = {};
    await expect
        .poll(
            async () => {
                const read = await readStyles(page);
                const now = read === null ? '' : JSON.stringify(read);
                const same = now !== '' && now === last;
                last = now;
                styles = read ?? {};
                return same;
            },
            { message: 'the styles of the page hold still', intervals: [250], timeout: 20_000 },
        )
        .toBe(true);
    return styles;
}

/** Every difference between `before` and `after`, in the order of the elements. */
export function compare(before: Elements, after: Elements): Change[] {
    const changes: Change[] = [];
    for (const key of Object.keys(before)) {
        if (!(key in after))
            changes.push({ key, property: 'element', before: 'shown', after: 'gone' });
    }
    for (const [key, styles] of Object.entries(after)) {
        const old = before[key];
        if (!old) {
            changes.push({ key, property: 'element', before: 'absent', after: 'shown' });
            continue;
        }
        const properties = new Set([...Object.keys(old), ...Object.keys(styles)]);
        for (const property of properties) {
            const was = old[property] ?? unset(property);
            const now = styles[property] ?? unset(property);
            if (was !== now) changes.push({ key, property, before: was, after: now });
        }
    }
    return changes;
}

/** A short text of the changes: one line per element, one per property under it. */
export function report(where: string, changes: Change[]): string {
    const count = `${changes.length} style change${changes.length === 1 ? '' : 's'}`;
    const lines = [`${where}: ${count}`];
    let element = '';
    for (const change of changes.slice(0, SHOWN)) {
        if (change.key !== element) lines.push(`  ${change.key}`);
        element = change.key;
        lines.push(`    ${change.property}: ${change.before} -> ${change.after}`);
    }
    if (changes.length > SHOWN) lines.push(`  and ${changes.length - SHOWN} more changes`);
    lines.push('When the change is on purpose, write the baseline again: STYLE_SNAPSHOT_UPDATE=1.');
    return lines.join('\n');
}

/** The baseline of one page, theme and width, or null when there is none. */
export function readBaseline(file: string): Snapshot | null {
    const at = join(BASELINES, file);
    if (!existsSync(at)) return null;
    return JSON.parse(readFileSync(at, 'utf8')) as Snapshot;
}

/** Write a baseline, as prettier formats JSON. */
export function writeBaseline(file: string, snapshot: Snapshot) {
    mkdirSync(BASELINES, { recursive: true });
    writeFileSync(join(BASELINES, file), `${JSON.stringify(snapshot, null, 2)}\n`);
}

/** The baseline files that no page, theme and width of the spec writes. */
export function staleBaselines(expected: string[]): string[] {
    if (!existsSync(BASELINES)) return [];
    return readdirSync(BASELINES).filter((file) => !expected.includes(file));
}

export function removeBaseline(file: string) {
    rmSync(join(BASELINES, file));
}
