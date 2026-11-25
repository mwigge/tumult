import { render } from 'svelte/server';
import { describe, expect, it } from 'vitest';

import Badge from './Badge.svelte';
import StatusBadge from './StatusBadge.svelte';

const html = (props: Record<string, unknown>) =>
    render(Badge, { props: props as never }).body.replace(/<!--[^>]*-->/g, '');

describe('a badge', () => {
    it('is a span of the badge class and the tone, with the word as its only child', () => {
        expect(html({ tone: 'warn', label: 'critical' })).toBe(
            '<span class="badge warn">critical</span>',
        );
    });

    it('takes its tone from the kind and the value, and shows the value', () => {
        expect(html({ kind: 'finding', value: 'low' })).toBe(
            '<span class="badge neutral">low</span>',
        );
        expect(html({ kind: 'finding', value: 'high' })).toBe(
            '<span class="badge fail">high</span>',
        );
    });

    it('shows its label in place of the value', () => {
        expect(html({ kind: 'verdict', value: 'not_recovered', label: 'not recovered' })).toBe(
            '<span class="badge fail">not recovered</span>',
        );
    });

    it('puts big before the tone and a class of the page after it, and keeps other attributes', () => {
        const out = html({
            kind: 'verdict',
            value: 'regressed',
            big: true,
            class: 'origin',
            title: 'why',
            'data-testid': 'verdict-badge',
        });
        expect(out).toContain('class="badge big fail origin"');
        expect(out).toContain('title="why"');
        expect(out).toContain('data-testid="verdict-badge"');
        expect(out).toMatch(/>regressed<\/span>$/);
    });

    it('of a status says unknown when there is none', () => {
        const status = (s: string | null) =>
            render(StatusBadge, { props: { status: s } }).body.replace(/<!--[^>]*-->/g, '');
        expect(status(null)).toBe('<span class="badge neutral">unknown</span>');
        expect(status('COMPLETED')).toBe('<span class="badge ok">COMPLETED</span>');
        expect(status('deviated')).toBe('<span class="badge fail">deviated</span>');
    });
});
