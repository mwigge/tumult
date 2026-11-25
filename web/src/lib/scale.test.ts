import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

import { build } from '../../scripts/build-tokens.mjs';
import { tokens } from './tokens.gen';

// The scales of Krönika. Type has four body sizes and three display sizes.
// Spacing is on a 4 px scale, with a 2 px half step for badges and chips.
// Radius has three steps, a pill and a round. The tests read the tokens as
// the generator writes them from web/tokens/.

const TOKENS = fileURLToPath(new URL('../../tokens', import.meta.url));

/** The body sizes: caps labels and table heads, small text, body, and emphasis and values. */
const BODY = [11, 12, 13, 14];
/** The display sizes: titles, scores and the hero number. */
const DISPLAY = [18, 24, 32];
/** The sizes that only a chart draws, on its canvas. */
const CANVAS = [9, 10];

/**
 * The custom properties at :root of tokens.css whose name starts with
 * `prefix`, as the generator writes them: name to value.
 */
function root(prefix: string): Record<string, string> {
    const css = build(TOKENS)['src/lib/tokens.css'];
    const block = /^:root \{([^}]*)\}/m.exec(css)?.[1] ?? '';
    return Object.fromEntries(
        [...block.matchAll(/--([\w-]+): ([^;]+);/g)]
            .filter(([, name]) => name.startsWith(prefix))
            .map(([, name, value]) => [name, value]),
    );
}

/** The number of px of a value such as 12px, or NaN when it is not in px. */
const px = (value: string) => (/^\d+(?:\.\d+)?px$/.test(value) ? parseFloat(value) : NaN);

describe('the type scale', () => {
    it('has four body sizes and three display sizes, each with one token', () => {
        const sizes = Object.values(root('font-size-'))
            .map(px)
            .sort((a, b) => a - b);

        expect(sizes.filter((size) => !(size >= DISPLAY[0]))).toEqual(BODY);
        expect(sizes.filter((size) => size >= DISPLAY[0])).toEqual(DISPLAY);
    });

    it('gives a chart the body sizes, and the canvas sizes 9 and 10 px', () => {
        for (const theme of Object.values(tokens)) {
            const sizes = Object.values(theme.chart.font.size) as number[];

            expect(sizes.filter((size) => ![...CANVAS, ...BODY].includes(size))).toEqual([]);
        }
    });

    it('has a line height for body text and a tight one', () => {
        expect(root('line-height-')).toEqual({
            'line-height-body': '1.5',
            'line-height-tight': '1',
        });
    });
});

describe('the spacing scale', () => {
    it('puts every spacing token on the 4 px scale, a multiple of 2', () => {
        const spacing = root('space-');

        expect(Object.keys(spacing).length).toBeGreaterThan(0);
        for (const [name, value] of Object.entries(spacing)) {
            // space-3 is step 3, 12px; space-0-5 is step 0.5, 2px.
            const step = Number(name.slice('space-'.length).replace(/-5$/, '.5'));

            expect(px(value) % 2, `${name} is ${value}`).toBe(0);
            expect(px(value), `${name} is ${value}`).toBe(step * 4);
        }
    });

    it('has one half step, 2 px, for badges and chips', () => {
        const halves = Object.keys(root('space-')).filter((name) => /^space-\d+-5$/.test(name));

        expect(halves).toEqual(['space-0-5']);
    });
});

describe('the radius scale', () => {
    it('has three steps, a pill and a round', () => {
        expect(root('radius-')).toEqual({
            'radius-xs': '2px',
            'radius-md': '4px',
            'radius-xl': '6px',
            'radius-pill': '999px',
            'radius-round': '50%',
        });
    });
});
