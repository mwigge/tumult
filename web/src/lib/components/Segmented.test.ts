import { render } from 'svelte/server';
import { describe, expect, it } from 'vitest';

import Segmented from './Segmented.svelte';

const html = (props: Record<string, unknown>) =>
    render(Segmented, {
        props: { label: 'Period', value: '90d', onchange: () => {}, ...props } as never,
    }).body.replace(/<!--[^>]*-->/g, '');

const PERIODS = [
    { value: '90d', label: '90 days' },
    { value: '1y', label: '1 year' },
];

describe('a segmented control', () => {
    it('is a group with the name it is given', () => {
        expect(html({ options: PERIODS })).toMatch(
            /^<div class="seg" role="group" aria-label="Period">/,
        );
    });

    it('says on each button whether it is pressed, and marks the chosen one active', () => {
        expect(html({ options: PERIODS })).toBe(
            '<div class="seg" role="group" aria-label="Period">' +
                '<button aria-pressed="true" class="active">90 days</button> ' +
                '<button aria-pressed="false">1 year</button></div>',
        );
        expect(html({ options: PERIODS, value: '1y' })).toContain(
            '<button aria-pressed="false">90 days</button> <button aria-pressed="true" class="active">1 year</button>',
        );
    });

    it('shows a value as its own label', () => {
        expect(html({ options: ['24h', '7d'], value: '7d', label: 'time range' })).toContain(
            '<button aria-pressed="false">24h</button> <button aria-pressed="true" class="active">7d</button>',
        );
    });

    it('presses no button when no option has the value', () => {
        expect(html({ options: PERIODS, value: '3y' })).not.toContain('aria-pressed="true"');
    });

    it('keeps the class of a page and the class that marks the chosen button', () => {
        const out = html({
            options: ['org', 'dora'],
            value: 'dora',
            label: 'heatmap value',
            class: 'toggle',
            selectedClass: 'on',
        });
        expect(out).toMatch(/^<div class="toggle" role="group" aria-label="heatmap value">/);
        expect(out).toContain('<button aria-pressed="true" class="on">dora</button>');
        expect(out).not.toContain('active');
    });
});
