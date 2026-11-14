import { describe, expect, it } from 'vitest';
import type { StarNode } from '../types.js';
import { listNodes } from './list.js';

const node = (id: string, kind: string, label: string) => ({ id, kind, label }) as StarNode;

const nodes = [
    node('wl:demo/deployment/api', 'workload', 'api'),
    node('sys:demo/message-queue', 'system', 'message-queue'),
    node('org:', 'org', 'Company'),
    node('sys:demo/api-worker', 'system', 'api-worker'),
    node('ent:cif-0001', 'entity', 'cif-0001'),
];

describe('listNodes', () => {
    it('orders nodes from the centre outwards, then by label', () => {
        expect(listNodes(nodes, '').map((n) => n.id)).toEqual([
            'org:',
            'ent:cif-0001',
            'sys:demo/api-worker',
            'sys:demo/message-queue',
            'wl:demo/deployment/api',
        ]);
    });

    it('filters on label, id or kind, whatever the case', () => {
        expect(listNodes(nodes, 'QUEUE').map((n) => n.id)).toEqual(['sys:demo/message-queue']);
        expect(listNodes(nodes, 'cif-').map((n) => n.id)).toEqual(['ent:cif-0001']);
        expect(listNodes(nodes, ' workload ').map((n) => n.id)).toEqual(['wl:demo/deployment/api']);
        expect(listNodes(nodes, 'nothing')).toEqual([]);
    });

    it('does not reorder the list it is given', () => {
        const before = nodes.map((n) => n.id);
        listNodes(nodes, '');
        expect(nodes.map((n) => n.id)).toEqual(before);
    });
});
