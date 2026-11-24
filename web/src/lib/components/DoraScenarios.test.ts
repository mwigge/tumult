import { render } from 'svelte/server';
import { describe, expect, it } from 'vitest';

import type { DoraScenario, DoraScenarios } from '../types';
import DoraScenariosView from './DoraScenarios.svelte';
import { badge, rowOf, textOf } from './dr-test-data';

const scenario = (over: Partial<DoraScenario>): DoraScenario => ({
    id: 'power_outage',
    state: 'missing',
    last_tested_at: null,
    last_test: null,
    last_passed_at: null,
    ...over,
});

const block = (over: Partial<DoraScenarios> = {}): DoraScenarios => ({
    source: 'Commission Delegated Regulation (EU) 2024/1774, Arts. 25, 26, 39 and 40',
    source_url: 'https://eur-lex.europa.eu/eli/reg_del/2024/1774/oj',
    rows: ['rts1774.art26'],
    window_days: 365,
    note: 'A scenario with no test is a gap, not a pass.',
    catalogue: [
        { id: 'cyber_attack', title: 'A cyber attack', cite: 'RTS 2024/1774 Art. 26(2)(a)' },
        { id: 'premises_loss', title: 'Premises fail', cite: 'RTS 2024/1774 Art. 26(2)(c)' },
        {
            id: 'power_outage',
            title: 'A widespread power outage',
            cite: 'RTS 2024/1774 Art. 26(2)(i)',
        },
    ],
    functions: [
        {
            entity_id: 'cif-0001',
            name: 'Payments',
            tier: 'critical',
            row: 'rts1774.art26',
            coverage: 0.4,
            counts: { passed: 1, failed: 1, overdue: 1, missing: 0 },
            scenarios: [
                scenario({
                    id: 'cyber_attack',
                    state: 'overdue',
                    last_tested_at: '2025-10-09',
                    last_test: { id: 'man-cyber', source: 'manual', outcome: 'passed' },
                    last_passed_at: '2025-10-09',
                }),
                scenario({
                    id: 'premises_loss',
                    state: 'failed',
                    last_tested_at: '2026-11-10',
                    last_test: { id: 'run-site', source: 'run', outcome: 'failed' },
                }),
                scenario({
                    id: 'power_outage',
                    state: 'passed',
                    last_tested_at: '2026-11-03',
                    last_test: { id: 'run-power', source: 'run', outcome: 'passed' },
                    last_passed_at: '2026-11-03',
                }),
            ],
        },
        {
            entity_id: 'cif-0002',
            name: 'Reporting',
            tier: 'important',
            row: 'rts1774.art26',
            coverage: 0,
            counts: { passed: 0, failed: 0, overdue: 0, missing: 3 },
            scenarios: [
                scenario({ id: 'cyber_attack' }),
                scenario({ id: 'premises_loss' }),
                scenario({ id: 'power_outage' }),
            ],
        },
    ],
    ...over,
});

const html = (scenarios: DoraScenarios) => render(DoraScenariosView, { props: { scenarios } }).body;

describe('the plan scenarios of the DORA view', () => {
    it('is a table of scenario rows by function columns with a caption', () => {
        const page = html(block());
        expect(page).toContain('<caption');
        expect(textOf(page)).toContain('Plan scenarios by function');
        expect(page).toContain('<th scope="col">Scenario</th>');
        for (const name of ['Payments', 'Reporting']) {
            expect(page).toMatch(new RegExp(`<th scope="col">${name}`));
        }
        expect(textOf(rowOf(page, 'data-scenario="power_outage"'))).toContain(
            'A widespread power outage power_outage · RTS 2024/1774 Art. 26(2)(i)',
        );
    });

    it('shows each state as a badge with the day of the newest test', () => {
        const page = html(block());
        const power = rowOf(page, 'data-scenario="power_outage"');
        expect(power).toMatch(badge('ok', 'passed'));
        expect(textOf(power)).toContain('passed tested 2026-11-03');
        const site = rowOf(page, 'data-scenario="premises_loss"');
        expect(site).toMatch(badge('fail', 'failed'));
        const cyber = rowOf(page, 'data-scenario="cyber_attack"');
        expect(cyber).toMatch(badge('warn', 'overdue'));
        expect(textOf(cyber)).toContain('overdue tested 2025-10-09');
        // Never tested is said as a gap, not left blank.
        expect(cyber).toMatch(badge('neutral', 'missing'));
    });

    it('counts what passed per function and says where the scenarios come from', () => {
        const page = html(block());
        expect(textOf(rowOf(page, 'data-counts'))).toContain('Passed in 365 days 1 of 3 0 of 3');
        expect(page).toContain('href="https://eur-lex.europa.eu/eli/reg_del/2024/1774/oj"');
        expect(textOf(page)).toContain('A scenario with no test is a gap, not a pass.');
        expect(textOf(page)).toContain('rts1774.art26');
    });

    it('says so when no critical or important function is on record', () => {
        const page = html(block({ functions: [] }));
        expect(page).not.toContain('<table');
        expect(textOf(page)).toContain('No critical or important function on record');
    });
});
