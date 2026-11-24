// Targets and tests of the DR view for the tests of the DR components.
import type { DrTarget, DrTest } from '../types';

export const DAY_MS = 86_400_000;
export const NOW_MS = Date.UTC(2026, 10, 23, 12);
export const ns = (ms: number) => ms * 1e6;

export function drTest(over: Partial<DrTest>): DrTest {
    return {
        test_id: 'automated:run-1',
        origin: 'automated',
        experiment_name: 'ledger failover',
        exercise_type: null,
        target_system: 'ledger',
        target_environment: 'prod',
        executed_at_ns: ns(NOW_MS - 10 * DAY_MS),
        outcome: 'completed',
        rto_target_s: 10,
        rto_measured_s: 4,
        rto_at_least_s: null,
        recovered: true,
        no_outage: false,
        rpo_target_s: null,
        rpo_measured_s: null,
        data_verified: null,
        verdict: 'met',
        evidence_id: 'run-1',
        reviewer: null,
        plan_id: null,
        ...over,
    };
}

export function drTarget(over: Partial<DrTarget>): DrTarget {
    return {
        system: 'ledger',
        environment: 'prod',
        state: 'ok',
        last_test: null,
        next_due_at_ns: null,
        interval_days: 365,
        interval_source: 'default',
        plan: null,
        rto_target_s: 10,
        rpo_target_s: null,
        rto_measured_s: null,
        rpo_measured_s: null,
        recovered: null,
        rto_at_least_s: null,
        ...over,
    };
}

/** The part of `html` from the element that carries `attr` to the end of its row. */
export function rowOf(html: string, attr: string): string {
    const start = html.indexOf(attr);
    if (start < 0) throw new Error(`no row ${attr}`);
    return html.slice(start, html.indexOf('</tr>', start));
}

/** `html` without its tags, with its white space made single. */
export const textOf = (html: string) =>
    html
        .replace(/<!--.*?-->/g, '')
        .replace(/<[^>]+>/g, ' ')
        .replace(/\s+/g, ' ')
        .trim();

/** The markup of a badge of `tone` that says `label`, with any scoping class. */
export const badge = (tone: string, label: string) =>
    new RegExp(`class="badge ${tone}(?: [^"]*)?">${label}<`);

/** What `html` holds after the start tag that carries `attr`. */
export function after(html: string, attr: string): string {
    const at = html.indexOf(attr);
    if (at < 0) throw new Error(`no ${attr}`);
    return html.slice(html.indexOf('>', at) + 1);
}
