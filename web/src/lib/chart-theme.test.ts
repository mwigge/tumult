import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

import { DARK, LIGHT, PALETTE, palette, themeSpec } from './chart-theme';
import { chartTokens } from './echarts';
import { tokens } from './tokens.gen';

const read = (file: string) => readFileSync(fileURLToPath(new URL(file, import.meta.url)), 'utf8');

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

/** The custom properties of one block of tokens.css: name without `--` to value. */
function properties(css: string, block: RegExp): Record<string, string> {
    const body = css.match(block)?.[1];
    if (body === undefined) throw new Error(`tokens.css has no block ${block}`);
    return Object.fromEntries(
        [...body.matchAll(/--([a-z-]+):\s*([^;]+);/g)].map(([, name, value]) => [name, value]),
    );
}

const CSS = read('./tokens.css');
const THEMES = {
    dark: {
        css: properties(CSS, /^:root \{([^}]*)\}/m),
        chart: chartTokens('tumult-dark'),
        roles: DARK,
    },
    light: {
        css: properties(CSS, /^:root\[data-theme='light'\] \{([^}]*)\}/m),
        chart: chartTokens('tumult-light'),
        roles: LIGHT,
    },
};

describe('the chart theme', () => {
    for (const [theme, { css, chart }] of Object.entries(THEMES)) {
        it(`draws in the colours the pages use, ${theme} theme`, () => {
            // A chart role and the custom property of the pages it stands for.
            expect({
                text: chart.text,
                axis: chart.axis,
                split: chart.split,
                accent: chart.accent,
                ok: chart.ok,
                warn: chart.warn,
                fail: chart.fail,
                tooltip: chart.tooltip.backgroundColor,
                'tooltip border': chart.tooltip.borderColor,
                'tooltip text': chart.tooltip.textStyle.color,
                bg: chart.bg,
                panel: chart.panel,
                hover: chart.hover,
                accentDim: chart.accentDim,
            }).toEqual({
                text: css['text-dim'],
                axis: css['border-strong'],
                split: css.border,
                accent: css.accent,
                ok: css.ok,
                warn: css.warn,
                fail: css.fail,
                tooltip: css['bg-panel'],
                'tooltip border': css['border-strong'],
                'tooltip text': css.text,
                bg: css.bg,
                panel: css['bg-panel'],
                hover: css['bg-hover'],
                accentDim: css['accent-dim'],
            });
        });
    }

    it('gives the auto theme of a light system the colours of the light theme', () => {
        const auto = properties(
            CSS,
            /@media \(prefers-color-scheme: light\) \{\s*:root\[data-theme='auto'\] \{([^}]*)\}/,
        );

        expect(auto).toEqual(THEMES.light.css);
    });

    it('starts its series with the accent of the dark theme', () => {
        expect(PALETTE[0]).toBe(THEMES.dark.css.accent);
        expect(PALETTE).toHaveLength(7);
    });

    it('starts the series of each theme with the accent of that theme', () => {
        expect(palette('tumult-dark')).toEqual(PALETTE);
        expect(palette('tumult-light')[0]).toBe(THEMES.light.css.accent);
        expect(palette('tumult-light')).toHaveLength(7);
        expect(themeSpec(LIGHT).color).toEqual(palette('tumult-light'));
    });

    it('has the same chart type and shared chart colours in each theme', () => {
        // chart-theme.ts reads these from the dark theme for both.
        const shared = [
            'font',
            'ink-on-fill',
            'neutral-dot',
            'band',
            'unscored',
            'status',
            'starmap',
        ];
        for (const key of shared as (keyof typeof tokens.dark.chart)[])
            expect(tokens.light.chart[key], key).toEqual(tokens.dark.chart[key]);
    });

    for (const [theme, { css, chart, roles }] of Object.entries(THEMES)) {
        /** Each of `colours` below 3:1 on the panel, with its ratio. */
        const below = (colours: readonly string[]) =>
            colours
                .map((colour) => `${colour} ${contrast(colour, css['bg-panel']).toFixed(2)}`)
                .filter((entry) => Number(entry.split(' ')[1]) < 3);

        it(`draws every series 3:1 or more against the panel, ${theme} theme`, () => {
            expect(below(themeSpec(roles).color), `on ${css['bg-panel']}`).toEqual([]);
        });

        it(`draws the extra series of the metrics chart 3:1 or more against the panel, ${theme} theme`, () => {
            expect(below(chart.extraSeries), `on ${css['bg-panel']}`).toEqual([]);
        });

        it(`writes chart text in the font of the page, ${theme} theme`, () => {
            const root = properties(CSS, /^:root \{([^}]*)\}/m);

            expect(themeSpec(roles).textStyle.fontFamily).toBe(root['font-body']);
        });
    }
});
