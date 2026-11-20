// The parameters of a run (`params_json`): a JSON object as the API sends
// it, or null for a run started without vars.

import type { RunRow } from './types';

type RunParams = RunRow['params_json'];

/** The vars the planned scope of a run is read with: its string parameters. */
export function planVars(params: RunParams): Record<string, string> {
    if (!params) return {};
    return Object.fromEntries(
        Object.entries(params).filter(
            (entry): entry is [string, string] => typeof entry[1] === 'string',
        ),
    );
}

/** The parameters as the approvals detail prints them: indented JSON. */
export function paramsText(params: RunParams): string {
    if (!params) return '—';
    return JSON.stringify(params, null, 2);
}
