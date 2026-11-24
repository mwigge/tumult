import { render } from 'svelte/server';
import { describe, expect, it } from 'vitest';

import DrFilters from './DrFilters.svelte';

const filters = (over: Record<string, unknown> = {}) =>
    render(DrFilters, {
        props: {
            filters: { target: 'ledger', environment: '', period: '90d' },
            systems: ['ledger', 'orders'],
            environments: ['prod', 'staging'],
            onchange: () => {},
            ...over,
        },
    }).body;

describe('the filters of the DR page', () => {
    it('offer every target and every environment, and all of them', () => {
        const html = filters();
        expect(html).toContain('aria-label="Target"');
        expect(html).toContain('aria-label="Environment"');
        for (const option of [
            'all targets',
            'ledger',
            'orders',
            'all environments',
            'prod',
            'staging',
        ]) {
            expect(html).toContain(`>${option}</option>`);
        }
    });

    it('keep a target the URL names that the register does not hold', () => {
        const html = filters({ filters: { target: 'gone', environment: 'dev', period: '1y' } });
        expect(html).toContain('>gone</option>');
        expect(html).toContain('>dev</option>');
    });

    it('offer the periods and mark the one in force', () => {
        const html = filters();
        expect(html).toContain('aria-label="Period"');
        expect(html).toMatch(/<button aria-pressed="true" class="active">90 days<\/button>/);
        expect(html).toMatch(/<button aria-pressed="false">1 year<\/button>/);
        expect(html).toMatch(/>3 years<\/button>/);
    });
});
