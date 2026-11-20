import { describe, expect, it } from 'vitest';
import { DARK, LIGHT } from '../chart-theme';
import { withColours } from './colours';

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
});
