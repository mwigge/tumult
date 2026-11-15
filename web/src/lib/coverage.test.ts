import { describe, expect, it } from 'vitest';
import { coverageLabels, disciplineLabel } from './coverage.js';

const row = (target: string, environment: string | null) => ({ target, environment });

describe('coverageLabels', () => {
    it('labels a row by its target when the name is unique', () => {
        expect(coverageLabels([row('api', 'prod'), row('queue', null)])).toEqual(['api', 'queue']);
    });

    it('adds the environment when one target is tested in several', () => {
        const rows = [row('api', 'prod'), row('api', 'staging'), row('queue', 'prod')];
        expect(coverageLabels(rows)).toEqual(['api (prod)', 'api (staging)', 'queue']);
    });

    it('names a missing environment rather than leaving two equal labels', () => {
        expect(coverageLabels([row('api', null), row('api', 'prod')])).toEqual([
            'api (no environment)',
            'api (prod)',
        ]);
    });
});

describe('disciplineLabel', () => {
    it('names performance as the load and performance discipline', () => {
        expect(disciplineLabel('performance')).toBe('load / performance');
    });

    it('names robustness as the robustness and disaster recovery discipline', () => {
        expect(disciplineLabel('robustness')).toBe('robustness / dr');
    });

    it('leaves every other name as it is', () => {
        expect(['chaos', 'security', 'dr'].map(disciplineLabel)).toEqual([
            'chaos',
            'security',
            'dr',
        ]);
    });
});
