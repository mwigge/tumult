import { readdirSync, readFileSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

import { render } from 'svelte/server';
import { describe, expect, it } from 'vitest';

import { PERIOD_DAYS, PERIOD_LABEL } from '../dr';
import DrFilters from './DrFilters.svelte';

const SRC = fileURLToPath(new URL('../..', import.meta.url));

/** The pages, components and modules of src/, without their tests. */
function sources(dir: string): string[] {
    return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
        const path = join(dir, entry.name);
        if (entry.isDirectory()) return sources(path);
        const code = /\.(svelte|ts)$/.test(entry.name) && !entry.name.endsWith('.test.ts');
        return code ? [relative(SRC, path)] : [];
    });
}

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

    it('name the periods from the one list in dr.ts that the Reports page reads too', () => {
        const buttons = [...filters().matchAll(/<button[^>]*>([^<]*)<\/button>/g)];
        expect(buttons.map((button) => button[1])).toEqual(
            Object.keys(PERIOD_DAYS).map((period) => PERIOD_LABEL[period]),
        );
        const names = Object.values(PERIOD_LABEL).map((label) => `'${label}'`);
        const writers = sources(SRC).filter((file) => {
            const source = readFileSync(join(SRC, file), 'utf8');
            return names.some((name) => source.includes(name));
        });
        expect(writers).toEqual([join('lib', 'dr.ts')]);
    });
});
