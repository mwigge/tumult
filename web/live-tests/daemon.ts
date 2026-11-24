// A real daemon for the tests of Krönika: `tumultd` with a home and a lake
// of its own, on ports of the loopback that are free.
import { spawn } from 'node:child_process';
import { mkdtempSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:net';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { expect } from '@playwright/test';

export interface Daemon {
    /** The home of the daemon: its working directory and its lake. */
    home: string;
    /** Where the daemon serves the pages and the API. */
    base: string;
    /** The environment of a binary: this home, and nothing of the machine but its PATH. */
    env(more?: Record<string, string>): Record<string, string>;
    stop(): void;
}

export function binary(name: 'TUMULT_BIN' | 'TUMULTD_BIN'): string {
    const path = process.env[name];
    if (!path) throw new Error(`${name} must name the binary this test runs`);
    return path;
}

// A port of the loopback that is free now.
function freePort(): Promise<number> {
    return new Promise((resolve, reject) => {
        const server = createServer();
        server.once('error', reject);
        server.listen(0, '127.0.0.1', () => {
            const address = server.address();
            const port = typeof address === 'object' && address !== null ? address.port : 0;
            server.close(() => resolve(port));
        });
    });
}

/** Start `tumultd` in a new home and wait until it answers. `org` is the
    org file of the house, next to the lake, when the test needs one. */
export async function startDaemon(org?: string): Promise<Daemon> {
    const home = mkdtempSync(join(tmpdir(), 'tumult-live-'));
    if (org) writeFileSync(join(home, 'org.yaml'), org);
    const env = (more: Record<string, string> = {}) => ({
        PATH: process.env.PATH ?? '',
        HOME: home,
        ...more,
    });
    const [grpc, http] = [await freePort(), await freePort()];
    const base = `http://127.0.0.1:${http}`;
    const child = spawn(binary('TUMULTD_BIN'), [], {
        cwd: home,
        env: env({
            TUMULT_LAKE_PATH: join(home, 'lake.duckdb'),
            KRONIKA_OTLP_GRPC_ADDR: `127.0.0.1:${grpc}`,
            KRONIKA_OTLP_HTTP_ADDR: `127.0.0.1:${http}`,
        }),
        stdio: ['ignore', 'pipe', 'pipe'],
    });
    let log = '';
    child.stdout?.on('data', (chunk) => (log += String(chunk)));
    child.stderr?.on('data', (chunk) => (log += String(chunk)));
    await expect
        .poll(
            async () => {
                if (child.exitCode !== null) throw new Error(`tumultd ended:\n${log}`);
                const answer = await fetch(`${base}/api/me`).catch(() => null);
                return answer?.status ?? 0;
            },
            { message: 'the daemon answers', timeout: 60_000 },
        )
        .toBe(200);
    return { home, base, env, stop: () => void child.kill('SIGTERM') };
}
