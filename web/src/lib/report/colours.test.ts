import { readFileSync } from 'node:fs';

import { describe, expect, it } from 'vitest';
import { DARK, LIGHT } from '../chart-theme';
import { COLOURS, withColours } from './colours';

/** The colours of the CLI report, which `npm run build:tokens` writes. */
const REPORT_CSS = new URL(
    '../../../../tumult-cli/src/commands/report/html/tokens.css',
    import.meta.url,
);

describe('withColours', () => {
    it('gives a named colour the colour of the theme', () => {
        const option = { series: [{ itemStyle: { color: 'token:ok' } }] };

        expect(withColours(option, LIGHT)).toEqual({
            series: [{ itemStyle: { color: '#0f8a5f' } }],
        });
        expect(withColours(option, DARK)).toEqual({
            series: [{ itemStyle: { color: '#34d399' } }],
        });
    });

    it('finds a colour in a list of data items', () => {
        const option = {
            series: [{ data: [{ value: 1, itemStyle: { color: 'token:fail' } }, 2, null] }],
        };

        expect(withColours(option, LIGHT)).toEqual({
            series: [{ data: [{ value: 1, itemStyle: { color: '#c0392b' } }, 2, null] }],
        });
    });

    it('finds the colours of a gradient', () => {
        const stops = (from: string, to: string) => ({
            areaStyle: {
                color: {
                    type: 'linear',
                    colorStops: [
                        { offset: 0, color: from },
                        { offset: 1, color: to },
                    ],
                },
            },
        });

        expect(withColours(stops('token:accent', 'transparent'), LIGHT)).toEqual(
            stops('#1f6fb2', 'transparent'),
        );
    });

    it('leaves a text that reads like a token alone', () => {
        const option = { yAxis: { data: ['token:ok'] }, series: [{ name: 'token:fail' }] };

        expect(withColours(option, LIGHT)).toEqual(option);
    });

    it('leaves a colour that is not a token alone', () => {
        const option = { color: ['#123456', 'transparent'], itemStyle: { color: 'transparent' } };

        expect(withColours(option, LIGHT)).toEqual(option);
    });

    it('leaves a token of an unknown name as written', () => {
        const option = { itemStyle: { color: 'token:nonsense' } };

        expect(withColours(option, LIGHT)).toEqual(option);
    });

    it('does not take a token that names more than a colour', () => {
        // `tooltip` is a part of the theme, not a colour.
        const option = { itemStyle: { color: 'token:tooltip' } };

        expect(withColours(option, LIGHT)).toEqual(option);
    });

    it('does not change the option it was given', () => {
        const option = { itemStyle: { color: 'token:warn' } };

        withColours(option, DARK);

        expect(option).toEqual({ itemStyle: { color: 'token:warn' } });
    });

    it('knows each colour of the CSS of the report by its name, in light and in dark', () => {
        // Rust names a colour of a chart by the name the CSS of the report
        // gives it, so the script must give it the same colour.
        const [light, rest] = readFileSync(REPORT_CSS, 'utf8').split(
            '@media (prefers-color-scheme: dark)',
        );
        const dark = rest.split('@media print')[0];

        for (const [theme, block] of [
            ['light', light],
            ['dark', dark],
        ] as const) {
            const declared = [...block.matchAll(/--([a-z-]+): (#[0-9a-f]{6});/g)];
            expect(declared.length, theme).toBeGreaterThanOrEqual(9);
            for (const [, name, value] of declared)
                expect(withColours({ color: `token:${name}` }, COLOURS[theme]), name).toEqual({
                    color: value,
                });
        }
    });
});
