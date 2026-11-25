import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

import { DARK, LIGHT, PALETTE } from './chart-theme';
import { chartTokens } from './echarts';

const read = (file: string) => readFileSync(fileURLToPath(new URL(file, import.meta.url)), 'utf8');

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

    it('has no colour written by hand in chart-theme.ts or echarts.ts', () => {
        for (const file of ['./chart-theme.ts', './echarts.ts']) {
            expect(read(file), file).not.toMatch(/#[0-9a-fA-F]{3,8}\b/);
        }
    });
});
