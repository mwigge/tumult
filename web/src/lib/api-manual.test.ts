// The client functions of the manual registry: the request each one sends,
// and the reason of a refusal as the API gives it. `fetch` is the edge of
// the client, so it stands in for the daemon here; the live tests send the
// same requests to a real one.
import { afterEach, describe, expect, it, vi } from 'vitest';

import { api, ApiError } from './api';

interface Sent {
    url: string;
    method: string;
    contentType: string | null;
    body: unknown;
}

function answer(status: number, body: BodyInit, type = 'application/json'): Sent[] {
    const sent: Sent[] = [];
    vi.stubGlobal('fetch', async (url: string, init: RequestInit = {}) => {
        const headers = new Headers(init.headers);
        sent.push({
            url,
            method: init.method ?? 'GET',
            contentType: headers.get('content-type'),
            body: init.body,
        });
        return new Response(body, { status, headers: { 'content-type': type } });
    });
    return sent;
}

const json = (value: unknown) => JSON.stringify(value);

afterEach(() => {
    vi.unstubAllGlobals();
});

describe('a file attached to a record', () => {
    it('is sent as its bytes, with its name and who adds it in the query', async () => {
        const sent = answer(201, json({ id: 'att-1', media_type: 'application/pdf' }));
        const file = new File(['%PDF-1.4 runbook'], 'run log.pdf', { type: 'application/pdf' });
        const added = await api.manualAttachFile('rec 1', file, 'alice');
        expect(added).toMatchObject({ id: 'att-1' });
        expect(sent).toEqual([
            {
                url: '/api/manual/experiments/rec%201/attachments?name=run+log.pdf&added_by=alice',
                method: 'POST',
                contentType: 'application/octet-stream',
                body: file,
            },
        ]);
    });

    it('leaves out who adds it when no name is given', async () => {
        const sent = answer(201, json({ id: 'att-1' }));
        await api.manualAttachFile('rec-1', new File(['x'], 'a.txt'), '');
        expect(sent[0].url).toBe('/api/manual/experiments/rec-1/attachments?name=a.txt');
    });

    it('fails with the reason the API gives', async () => {
        answer(415, json({ error: 'the file is not an allowed type', allowed: ['pdf'] }));
        const upload = api.manualAttachFile('rec-1', new File(['MZ'], 'tool.exe'), 'alice');
        await expect(upload).rejects.toEqual(new ApiError('the file is not an allowed type', 415));
    });

    it('comes back as its bytes from the download', async () => {
        const sent = answer(200, '%PDF-1.4 runbook', 'application/pdf');
        const file = await api.manualDownload('rec-1', 'att 1');
        expect(await file.text()).toBe('%PDF-1.4 runbook');
        expect(sent[0]).toMatchObject({
            url: '/api/manual/experiments/rec-1/attachments/att%201/file',
            method: 'GET',
        });
    });

    it('is not downloaded when the API refuses it, and the reason shows', async () => {
        answer(409, json({ error: 'the stored file no longer matches its hash' }));
        await expect(api.manualDownload('rec-1', 'att-1')).rejects.toThrow(
            'the stored file no longer matches its hash',
        );
    });
});

describe('a link attached to a record', () => {
    it('is sent as JSON with its kind, its address and its label', async () => {
        const sent = answer(201, json({ id: 'att-2' }));
        await api.manualAttachLink('rec-1', {
            kind: 'ticket',
            uri: 'https://tickets.example/OPS-7',
            label: 'OPS-7',
            added_by: 'alice',
        });
        expect(sent[0]).toMatchObject({
            url: '/api/manual/experiments/rec-1/attachments',
            method: 'POST',
            contentType: 'application/json',
        });
        expect(JSON.parse(String(sent[0].body))).toEqual({
            kind: 'ticket',
            uri: 'https://tickets.example/OPS-7',
            label: 'OPS-7',
            added_by: 'alice',
        });
    });
});

describe('a refusal in plain text', () => {
    it('shows the text, as axum answers a body it cannot read', async () => {
        const reason =
            'Failed to deserialize the JSON body into the target type: missing field `due_at_ns`';
        answer(422, reason, 'text/plain; charset=utf-8');
        await expect(
            api.manualPlanCreate({
                name: 'x',
                exercise_type: 'failover',
                target_system: null,
                target_environment: null,
                function_id: null,
                scope: null,
                owner: 'alice',
                planned_at_ns: null,
                interval_days: null,
                rto_target_s: null,
                rpo_target_s: null,
                created_by: 'alice',
            }),
        ).rejects.toEqual(new ApiError(reason, 422));
    });

    it('falls back to the status when the answer says nothing', async () => {
        answer(503, '', 'text/plain');
        await expect(api.manualAuditVerify('rec-1')).rejects.toEqual(new ApiError('HTTP 503', 503));
    });
});

describe('the plans', () => {
    it('are listed, all of them or those of one status', async () => {
        const sent = answer(200, json({ plans: [] }));
        await api.manualPlans();
        await api.manualPlans('planned');
        expect(sent.map((s) => s.url)).toEqual([
            '/api/manual/plans',
            '/api/manual/plans?status=planned',
        ]);
    });

    it('are changed with a PATCH and cancelled with a reason', async () => {
        const sent = answer(200, json({ ok: true }));
        await api.manualPlanEdit('plan 1', { owner: 'bob', updated_by: 'bob' });
        await api.manualPlanCancel('plan-1', 'bob', 'the system is retired');
        expect(sent.map((s) => [s.method, s.url])).toEqual([
            ['PATCH', '/api/manual/plans/plan%201'],
            ['POST', '/api/manual/plans/plan-1/cancel'],
        ]);
        expect(JSON.parse(String(sent[0].body))).toEqual({ owner: 'bob', updated_by: 'bob' });
        expect(JSON.parse(String(sent[1].body))).toEqual({
            by: 'bob',
            reason: 'the system is retired',
        });
    });
});

describe('one plan', () => {
    it('is read with its audit trail, and verified as a record is', async () => {
        const sent = answer(200, json({ plan: { id: 'plan 1' }, audit: [] }));
        expect(await api.manualPlanDetail('plan 1')).toEqual({ plan: { id: 'plan 1' }, audit: [] });
        await api.manualPlanAuditVerify('plan 1');
        expect(sent.map((s) => [s.method, s.url])).toEqual([
            ['GET', '/api/manual/plans/plan%201'],
            ['GET', '/api/manual/plans/plan%201/audit/verify'],
        ]);
    });
});

describe('the audit verify and the import', () => {
    it('reads the verdict of one record', async () => {
        const verdict = { experiment_id: 'rec-1', ok: true, problems: [] };
        const sent = answer(200, json(verdict));
        expect(await api.manualAuditVerify('rec-1')).toEqual(verdict);
        expect(sent[0]).toMatchObject({
            url: '/api/manual/experiments/rec-1/audit/verify',
            method: 'GET',
        });
    });

    it('posts the body of an import', async () => {
        const sent = answer(201, json({ batch_id: 'b-1', ids: ['r-1'] }));
        const body = { label: 'q3', records: [{ experiment_name: 'a' }] };
        expect(await api.manualImport(body)).toEqual({ batch_id: 'b-1', ids: ['r-1'] });
        expect(sent[0]).toMatchObject({ url: '/api/manual/import', method: 'POST' });
        expect(JSON.parse(String(sent[0].body))).toEqual(body);
    });
});
