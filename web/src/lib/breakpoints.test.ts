import { readdirSync, readFileSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

// The widths where the layout of Krönika changes have names, in one place:
// the list of breakpoints in theme.css. A media query cannot read a custom
// property, so each query writes its width, and this test holds every
// query of the pages, the components and theme.css to the list.

const SRC = fileURLToPath(new URL('..', import.meta.url));

/** The breakpoints, by name, as theme.css lists them. */
const BREAKPOINTS = { xl: 1200, lg: 1100, md: 900, sm: 800 };

/** The breakpoints that the list in theme.css names: `name width` per line. */
function listed(css: string): Record<string, number> {
    const block = css.match(/\/\* --- breakpoints -[\s\S]*?\*\//)?.[0] ?? '';
    return Object.fromEntries(
        [...block.matchAll(/^\s+([a-z]+)\s+(\d+)px\s/gm)].map(([, name, px]) => [name, Number(px)]),
    );
}

/** The widths that the media queries of CSS text ask for, as written. */
const widthsOf = (css: string) =>
    [...css.matchAll(/@media[^{]*/g)].flatMap(([prelude]) =>
        [...prelude.matchAll(/width\s*(?::|[<>]=?)\s*([\d.]+[a-z]*)/g)].map(([, width]) => width),
    );

/** The CSS of `file`: a CSS file, or the <style> blocks of a Svelte file. */
function cssOf(file: string): string {
    const text = readFileSync(join(SRC, file), 'utf8');
    if (file.endsWith('.css')) return text;
    return [...text.matchAll(/<style[^>]*>([\s\S]*?)<\/style>/g)].map((m) => m[1]).join('\n');
}

/** The Svelte and CSS files under `dir`, as paths from src/. */
function styleFiles(dir: string): string[] {
    return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
        const path = join(dir, entry.name);
        if (entry.isDirectory()) return styleFiles(path);
        return /\.(?:svelte|css)$/.test(entry.name) ? [relative(SRC, path)] : [];
    });
}

describe('the scan for breakpoints', () => {
    it('reads the list and the widths of the queries', () => {
        const css = [
            '/* --- breakpoints -----',
            '     xl  1200px  five key numbers go three in a row',
            '     sm   800px  two in a row',
            '   */',
            '@media (max-width: 800px) { .a { gap: 0 } }',
            '@media (min-width: 61em) and (width <= 700px) { .b { gap: 0 } }',
            '@media (prefers-color-scheme: light) { .c { gap: 0 } }',
        ].join('\n');

        expect(listed(css)).toEqual({ xl: 1200, sm: 800 });
        expect(widthsOf(css)).toEqual(['800px', '61em', '700px']);
    });
});

describe('the breakpoints', () => {
    it('are named once, in the list in theme.css', () => {
        expect(listed(cssOf('lib/theme.css'))).toEqual(BREAKPOINTS);
    });

    it('are the only widths that a media query asks for', () => {
        const named = Object.values(BREAKPOINTS).map((px) => `${px}px`);
        const wrong = styleFiles(SRC)
            .sort()
            .flatMap((file) =>
                widthsOf(cssOf(file))
                    .filter((width) => !named.includes(width))
                    .map((width) => `${file}: ${width}`),
            );

        expect(wrong, `use a breakpoint of theme.css: ${named.join(', ')}`).toEqual([]);
    });
});
