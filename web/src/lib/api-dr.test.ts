// The client function of the DR view: the query it sends, and the reason
// of a refusal as the API gives it. `fetch` is the edge of the client, so
// it stands in for the daemon here; the live test sends the same requests
// to a real one.
import { afterEach, describe, expect, it, vi } from 'vitest';

import { api, ApiError } from './api';

function answer(status: number, body: unknown): string[] {
    const sent: string[] = [];
    vi.stubGlobal('fetch', async (url: string) => {
        sent.push(url);
        return new Response(JSON.stringify(body), {
            status,
            headers: { 'content-type': 'application/json' },
        });
    });
    return sent;
}

afterEach(() => {
    vi.unstubAllGlobals();
});

describe('the DR view', () => {
    it('asks for the whole view when no filter is set', async () => {
        const sent = answer(200, { targets: [], tests: [] });
        const view = await api.dr();
        expect(view).toEqual({ targets: [], tests: [] });
        expect(sent).toEqual(['/api/dr']);
    });

    it('sends each filter, the period, the page size and the cursor', async () => {
        const sent = answer(200, {});
        await api.dr({
            target: 'payments db',
            environment: 'prod',
            from_ns: 1_700_000_000_000_000_000,
            to_ns: 1_800_000_000_000_000_000,
            limit: 20,
            cursor: 'a1b2',
        });
        expect(sent).toEqual([
            '/api/dr?target=payments+db&environment=prod&from=1700000000000000000' +
                '&to=1800000000000000000&limit=20&cursor=a1b2',
        ]);
    });

    it('leaves out a filter that is empty', async () => {
        const sent = answer(200, {});
        await api.dr({ target: '', environment: '', limit: 5 });
        expect(sent).toEqual(['/api/dr?limit=5']);
    });

    it('fails with the reason the API gives', async () => {
        answer(403, { error: 'environment prod is outside the principal scopes' });
        await expect(api.dr({ environment: 'prod' })).rejects.toEqual(
            new ApiError('environment prod is outside the principal scopes', 403),
        );
    });
});
