import { describe, expect, it } from 'vitest';

import { paramsText, planVars } from './run-params';

// `params_json` comes from the API as a JSON object, or null for a run
// started without vars (pinned by the API tests of tumult-api).

describe('the parameters of a run', () => {
    it('gives the vars of the object to read the planned scope with', () => {
        expect(planVars({ region: 'eu-north', replicas: '3' })).toEqual({
            region: 'eu-north',
            replicas: '3',
        });
    });

    it('leaves out a parameter that is not a string', () => {
        expect(planVars({ region: 'eu-north', depth: 3, dry: true })).toEqual({
            region: 'eu-north',
        });
    });

    it('gives no vars for a run without parameters', () => {
        expect(planVars(null)).toEqual({});
    });

    it('prints the object as indented JSON for the approvals detail', () => {
        const text = paramsText({ region: 'eu-north' });
        expect(text).toBe('{\n  "region": "eu-north"\n}');
        expect(text).not.toContain('[object Object]');
    });

    it('prints a dash for a run without parameters', () => {
        expect(paramsText(null)).toBe('—');
    });
});
