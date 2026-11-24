import { render } from 'svelte/server';
import { describe, expect, it } from 'vitest';

import DrTestTable from './DrTestTable.svelte';
import { badge, drTest, rowOf, textOf } from './dr-test-data';
import type { DrTest } from '../types';

const TESTS: DrTest[] = [
    drTest({
        test_id: 'automated:orders-dr',
        evidence_id: 'orders-dr',
        experiment_name: 'orders failover',
    }),
    drTest({
        test_id: 'automated:payments-dr',
        evidence_id: 'payments-dr',
        experiment_name: 'payments-db failover',
        target_system: 'payments-db',
        outcome: 'deviated',
        rto_measured_s: null,
        rto_at_least_s: 30.1,
        recovered: false,
        verdict: 'not_recovered',
    }),
    drTest({
        test_id: 'automated:search-dr',
        evidence_id: 'search-dr',
        experiment_name: null,
        target_environment: 'staging',
        rto_measured_s: null,
        recovered: null,
        no_outage: true,
        verdict: 'not_measured',
    }),
    drTest({
        test_id: 'manual:01CAT',
        origin: 'manual',
        evidence_id: '01CAT',
        exercise_type: 'restore',
        experiment_name: 'catalog restore',
        target_system: 'catalog',
        rto_target_s: 900,
        rto_measured_s: 300,
        rpo_target_s: 60,
        rpo_measured_s: 10,
        data_verified: true,
        reviewer: 'bob',
    }),
];

const table = (over: Record<string, unknown> = {}) =>
    render(DrTestTable, {
        props: {
            tests: TESTS,
            total: 45,
            page: 2,
            pageSize: 20,
            canPrev: true,
            canNext: false,
            ...over,
        },
    }).body;

describe('the table of DR tests', () => {
    it('is a table with a caption and column headers', () => {
        const html = table();
        expect(html).toContain('<caption');
        for (const column of ['Executed', 'Test', 'Origin', 'Target', 'Verdict', 'RTO', 'RPO']) {
            expect(html).toContain(`<th scope="col">${column}</th>`);
        }
    });

    it('links a run to its page and a manual record to the manual records', () => {
        const html = table();
        expect(rowOf(html, 'data-test="automated:orders-dr"')).toContain(
            'href="/experiments/orders-dr"',
        );
        const manual = rowOf(html, 'data-test="manual:01CAT"');
        expect(manual).toContain('href="/manual?tab=records"');
        expect(textOf(manual)).toContain('catalog restore');
        expect(textOf(manual)).toContain('manual restore');
        expect(textOf(rowOf(html, 'data-test="automated:search-dr"'))).toContain('search-dr');
    });

    it('shows each verdict and what each test measured', () => {
        const html = table();
        const payments = rowOf(html, 'data-test="automated:payments-dr"');
        expect(payments).toMatch(badge('fail', 'not recovered'));
        expect(textOf(payments)).toContain('at least 30.1 s, not recovered');
        const search = rowOf(html, 'data-test="automated:search-dr"');
        expect(search).toMatch(badge('neutral', 'not measured'));
        expect(textOf(search)).toContain('no outage seen');
        const manual = textOf(rowOf(html, 'data-test="manual:01CAT"'));
        expect(manual).toContain('5 min target 15 min');
        expect(manual).toContain('10 s target 60 s');
        expect(manual).toContain('yes');
        expect(manual).toContain('bob');
    });

    it('says which page it shows, and offers only the pages there are', () => {
        const html = table();
        expect(textOf(html)).toContain('Page 2: tests 21 to 24 of 45');
        expect(html).toMatch(/<button[^>]*>Previous<\/button>/);
        expect(html).toMatch(/<button[^>]*disabled[^>]*>Next<\/button>/);
        expect(table({ page: 1, canPrev: false, canNext: true })).toMatch(
            /<button[^>]*disabled[^>]*>Previous<\/button>/,
        );
    });

    it('says when the period has no DR test', () => {
        const html = table({ tests: [], total: 0, page: 1, canPrev: false });
        expect(textOf(html)).toContain('No DR test in this period.');
        expect(html).not.toContain('<table');
    });
});
