import { render } from 'svelte/server';
import { describe, expect, it } from 'vitest';

import DrRecovery from './DrRecovery.svelte';
import { DAY_MS, NOW_MS, after, drTarget, drTest, ns, textOf } from './dr-test-data';
import { recoveryGroups } from '../dr';

const period = { from_ns: ns(NOW_MS - 365 * DAY_MS), to_ns: ns(NOW_MS) };

const groups = recoveryGroups(
    [
        drTest({ test_id: 'automated:a' }),
        drTest({
            test_id: 'automated:b',
            recovered: false,
            rto_measured_s: null,
            rto_at_least_s: 30.1,
            verdict: 'not_recovered',
        }),
        drTest({
            test_id: 'manual:c',
            origin: 'manual',
            target_system: 'catalog',
            rto_target_s: 900,
            rto_measured_s: 300,
            rpo_target_s: 60,
            rpo_measured_s: 10,
        }),
    ],
    [drTarget({})],
);

const recovery = (over: Record<string, unknown> = {}) =>
    render(DrRecovery, { props: { groups, period, truncated: false, ...over } }).body;

describe('the recovery of each target over time', () => {
    it('has one part per target, with a chart of its RTO and of its RPO', () => {
        const html = recovery();
        expect(html.match(/<h3/g)).toHaveLength(2);
        expect(textOf(html)).toContain('catalog · prod');
        expect(textOf(html)).toContain('ledger · prod');
        expect(html).toContain('data-chart="dr-rto-0"');
        expect(html).toContain('data-chart="dr-rpo-0"');
        expect(html).toContain('aria-label="RTO of catalog in prod over the period"');
        expect(html).toContain('data-chart="dr-rto-1"');
    });

    it('says when a target measured no RPO in the period', () => {
        const html = recovery();
        expect(html).not.toContain('data-chart="dr-rpo-1"');
        expect(textOf(html)).toContain('No RPO measured in this period.');
    });

    it('explains the marks of the charts once, in words and in their colours', () => {
        const html = recovery();
        expect(html.match(/data-legend=/g)).toHaveLength(1);
        const legend = after(html, 'data-legend="dr-recovery"');
        expect(textOf(legend)).toMatch(
            /^measured ◆ manual test ▲ did not recover: the RTO was at least this target/,
        );
        expect(legend).toMatch(/<b style="color: #CC79A7;">◆<\/b>/);
        expect(legend).toMatch(/<b style="color: #ef4444;">▲<\/b>/);
    });

    it('says when the charts show only the newest tests of the period', () => {
        expect(textOf(recovery({ truncated: true }))).toContain(
            'The charts show the newest 1000 tests of the period.',
        );
        expect(textOf(recovery())).not.toContain('newest 1000');
    });

    it('says when the period has no DR test to draw', () => {
        expect(textOf(recovery({ groups: [] }))).toContain('No DR test in this period to draw.');
    });
});
