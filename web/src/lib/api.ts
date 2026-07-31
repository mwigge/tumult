// Fetch wrappers for the Tumult query API (same origin when embedded in
// tumultd; vite dev proxies /api to a local daemon).

import { goto } from '$app/navigation';
import type {
  ApprovalQueueRow,
  ApprovalTier,
  AskResponse,
  Dimensions,
  DryRunResponse,
  ExperimentDetail,
  ExperimentRow,
  ExperimentWindow,
  LogEntry,
  LoginResponse,
  LogVolume,
  ManualDetail,
  ManualExperiment,
  ManualRecordInput,
  MeResponse,
  MetricCatalogEntry,
  MetricDefInfo,
  MetricQueryResult,
  Overview,
  RegistryDefinition,
  RegistryEntry,
  ReportFile,
  ReportMetaV2,
  ReportTemplate,
  RunDetail,
  RunRow,
  Scorecard,
  ScoreTree,
  Timeseries,
  Topology,
  TraceDetail,
  TraceDurations,
  TraceRow
} from './types';

// A 401 on any non-auth endpoint means the session is gone (or absent) — send
// the user to /login, unless we are already there. Auth endpoints themselves
// (login failure, wrong current password) surface their 401 to the caller.
function redirectOnUnauthorized(resp: Response, path: string): void {
  if (
    resp.status === 401 &&
    !path.startsWith('/api/auth/') &&
    window.location.pathname !== '/login'
  ) {
    void goto('/login');
  }
}

async function get<T>(path: string): Promise<T> {
  const resp = await fetch(path);
  const body = await resp.json().catch(() => ({}));
  if (!resp.ok) {
    redirectOnUnauthorized(resp, path);
    throw new Error(body.error ?? `HTTP ${resp.status}`);
  }
  return body as T;
}

async function send<T>(method: string, path: string, payload: unknown): Promise<T> {
  const resp = await fetch(path, {
    method,
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(payload)
  });
  const body = await resp.json().catch(() => ({}));
  if (!resp.ok) {
    redirectOnUnauthorized(resp, path);
    throw new Error(body.error ?? `HTTP ${resp.status}`);
  }
  return body as T;
}

export const api = {
  login: (username: string, password: string) =>
    send<LoginResponse>('POST', '/api/auth/login', { username, password }),

  logout: () => send<{ ok: boolean }>('POST', '/api/auth/logout', {}),

  changePassword: (currentPassword: string, newPassword: string) =>
    send<{ changed: boolean }>('POST', '/api/auth/change-password', {
      current_password: currentPassword,
      new_password: newPassword
    }),

  me: () => get<MeResponse>('/api/me'),

  overview: (range: string) => get<Overview>(`/api/overview?range=${range}`),

  timeseries: (metric: string, interval: string, range: string) =>
    get<Timeseries>(`/api/timeseries?metric=${metric}&interval=${interval}&range=${range}`),

  experiments: (params: Record<string, string>) => {
    const qs = new URLSearchParams(
      Object.entries(params).filter(([, v]) => v !== '')
    ).toString();
    return get<{ count: number; experiments: ExperimentRow[] }>(
      `/api/experiments${qs ? `?${qs}` : ''}`
    );
  },

  experiment: (id: string) => get<ExperimentDetail>(`/api/experiments/${encodeURIComponent(id)}`),

  experimentWindows: (fromNs: number, toNs: number) =>
    get<{ count: number; runs: ExperimentWindow[] }>(
      `/api/experiments/windows?from=${fromNs}&to=${toNs}`
    ),

  dimensions: () => get<Dimensions>('/api/dimensions'),

  metrics: () => get<{ metrics: MetricDefInfo[] }>('/api/metrics'),

  reports: () => get<{ reports: ReportFile[] }>('/api/reports'),

  scores: (range: string) => get<Scorecard>(`/api/scores?range=${range}`),

  reportsV2: () => get<{ reports: ReportMetaV2[] }>('/api/reports/v2'),

  generateReportV2: (req: {
    type: ReportTemplate;
    period?: string;
    experiment_id?: string;
    framework?: string;
  }) => send<ReportMetaV2>('POST', '/api/reports/v2/generate', req),

  generateReport: (metric: string) =>
    send<{ name: string }>('POST', '/api/reports/generate', { metric }),

  logs: (params: Record<string, string>) => {
    const qs = new URLSearchParams(
      Object.entries(params).filter(([, v]) => v !== '')
    ).toString();
    return get<{ count: number; logs: LogEntry[] }>(`/api/logs${qs ? `?${qs}` : ''}`);
  },

  logsVolume: (params: Record<string, string>) => {
    const qs = new URLSearchParams(
      Object.entries(params).filter(([, v]) => v !== '')
    ).toString();
    return get<LogVolume>(`/api/logs/volume${qs ? `?${qs}` : ''}`);
  },

  traces: (params: Record<string, string>) => {
    const qs = new URLSearchParams(
      Object.entries(params).filter(([, v]) => v !== '')
    ).toString();
    return get<{ count: number; traces: TraceRow[] }>(`/api/traces${qs ? `?${qs}` : ''}`);
  },

  traceDurations: (range: string) => get<TraceDurations>(`/api/traces/durations?range=${range}`),

  trace: (id: string) => get<TraceDetail>(`/api/traces/${encodeURIComponent(id)}`),

  metricsCatalog: () => get<{ metrics: MetricCatalogEntry[] }>('/api/metrics/catalog'),

  metricQuery: (params: Record<string, string>) => {
    const qs = new URLSearchParams(
      Object.entries(params).filter(([, v]) => v !== '')
    ).toString();
    return get<MetricQueryResult>(`/api/metrics/query${qs ? `?${qs}` : ''}`);
  },

  topology: (range: string) => get<Topology>(`/api/topology?range=${range}`),

  ask: async (question: string): Promise<AskResponse> => {
    // Deliberately not routed through send(): a non-ok response whose body
    // carries `configured` (LLM not set up) is a valid answer, not an error.
    const resp = await fetch('/api/ask', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ question })
    });
    const body = await resp.json().catch(() => ({}));
    if (!resp.ok && !('configured' in body)) {
      redirectOnUnauthorized(resp, '/api/ask');
      // Carry the status so callers can special-case e.g. 422 (scoped user
      // asking a question that can't be confined to their environments).
      const err = new Error(body.error ?? `HTTP ${resp.status}`) as Error & { status?: number };
      err.status = resp.status;
      throw err;
    }
    return body as AskResponse;
  },

  scoreTree: (node: string, range: string) =>
    get<ScoreTree>(`/api/scores/tree?node=${encodeURIComponent(node)}&range=${range}`),

  manualList: (status: string) =>
    get<{ records: ManualExperiment[] }>(
      `/api/manual/experiments${status ? `?status=${status}` : ''}`
    ),

  manualDetail: (id: string) =>
    get<ManualDetail>(`/api/manual/experiments/${encodeURIComponent(id)}`),

  manualCreate: (rec: ManualRecordInput) =>
    send<{ id: string }>('POST', '/api/manual/experiments', rec),

  manualUpdate: (id: string, rec: ManualRecordInput) =>
    send<{ ok: boolean }>('PUT', `/api/manual/experiments/${encodeURIComponent(id)}`, rec),

  manualSubmit: (id: string, by: string, attestation?: string) =>
    send<{ ok: boolean }>('POST', `/api/manual/experiments/${encodeURIComponent(id)}/submit`, {
      by,
      attestation: attestation ?? null
    }),

  manualVerify: (id: string, reviewer: string, note?: string) =>
    send<{ ok: boolean }>('POST', `/api/manual/experiments/${encodeURIComponent(id)}/verify`, {
      reviewer,
      note: note ?? null
    }),

  manualReject: (id: string, reviewer: string, note: string) =>
    send<{ ok: boolean }>('POST', `/api/manual/experiments/${encodeURIComponent(id)}/reject`, {
      reviewer,
      note
    }),

  manualAttach: (id: string, kind: string, uri: string, label: string | null, addedBy: string) =>
    send<{ id: string }>(
      'POST',
      `/api/manual/experiments/${encodeURIComponent(id)}/attachments`,
      { kind, uri, label, added_by: addedBy }
    ),

  manualImport: (label: string | null, records: ManualRecordInput[]) =>
    send<{ batch_id: string; ids: string[] }>('POST', '/api/manual/import', { label, records }),

  // --- UI execution: registry + runs ----------------------------------------

  registry: () => get<{ count: number; definitions: RegistryEntry[] }>('/api/registry'),

  registryDefinition: (id: string) =>
    get<{ definition: RegistryDefinition }>(`/api/registry/${encodeURIComponent(id)}`),

  dryRun: (registry_id: string, vars: Record<string, string>) =>
    send<DryRunResponse>('POST', '/api/runs/dry-run', { registry_id, vars }),

  startRun: (registry_id: string, vars: Record<string, string>) =>
    send<{ run_id: string; state: string; tier?: ApprovalTier }>('POST', '/api/runs', {
      registry_id,
      vars
    }),

  runs: (state?: string, limit = 100) => {
    const qs = new URLSearchParams();
    if (state) qs.set('state', state);
    qs.set('limit', String(limit));
    return get<{ count: number; runs: RunRow[] }>(`/api/runs?${qs}`);
  },

  run: (id: string) => get<RunDetail>(`/api/runs/${encodeURIComponent(id)}`),

  stopRun: (id: string) =>
    send<{ run_id: string; stop: string }>('POST', `/api/runs/${encodeURIComponent(id)}/stop`, {}),

  // --- T10: approval workflow -------------------------------------------------

  approvals: () => get<{ count: number; queue: ApprovalQueueRow[] }>('/api/approvals'),

  approveRun: (id: string, note?: string) =>
    send<{ run_id: string; state: 'queued' | 'pending_approval' }>(
      'POST',
      `/api/runs/${encodeURIComponent(id)}/approve`,
      { note: note ?? null }
    ),

  rejectRun: (id: string, note?: string) =>
    send<{ run_id: string; state: 'rejected' }>(
      'POST',
      `/api/runs/${encodeURIComponent(id)}/reject`,
      { note: note ?? null }
    ),

  breakGlass: (id: string, justification: string) =>
    send<{ run_id: string; state: 'queued'; break_glass: boolean }>(
      'POST',
      `/api/runs/${encodeURIComponent(id)}/break-glass`,
      { justification }
    )
};

// --- formatting helpers ----------------------------------------------------

export function fmtKpi(value: number | null, unit: string): string {
  if (value === null || value === undefined || Number.isNaN(value)) return '—';
  if (unit === 'ratio') return `${(value * 100).toFixed(1)}%`;
  if (unit === 'seconds') return `${value.toFixed(1)}s`;
  return `${Math.round(value)}`;
}

export function fmtDelta(delta: number | null, unit: string): { text: string; cls: string } {
  if (delta === null || delta === undefined || Number.isNaN(delta)) {
    return { text: '', cls: 'flat' };
  }
  const eps = unit === 'ratio' ? 0.0005 : 0.5;
  const text =
    unit === 'ratio'
      ? `${delta >= 0 ? '+' : ''}${(delta * 100).toFixed(1)}pp`
      : `${delta >= 0 ? '+' : ''}${Math.round(delta)}`;
  return { text, cls: delta > eps ? 'up' : delta < -eps ? 'down' : 'flat' };
}

export function fmtTs(ns: number): string {
  return new Date(ns / 1_000_000).toLocaleString(undefined, {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit'
  });
}

export function fmtAgo(ns: number): string {
  const s = Math.max(0, (Date.now() - ns / 1_000_000) / 1000);
  if (s < 60) return `${Math.round(s)}s ago`;
  if (s < 3600) return `${Math.round(s / 60)}m ago`;
  if (s < 86400) return `${Math.round(s / 3600)}h ago`;
  return `${Math.round(s / 86400)}d ago`;
}

export function fmtDuration(ns: number | null): string {
  if (ns === null || ns === undefined) return '—';
  const ms = ns / 1_000_000;
  if (ms < 1000) return `${Math.round(ms)}ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`;
  return `${Math.floor(ms / 60_000)}m ${Math.round((ms % 60_000) / 1000)}s`;
}

export function shortId(id: string): string {
  return id.length > 8 ? id.slice(0, 8) : id;
}

export function statusClass(status: string | null): 'ok' | 'warn' | 'fail' | 'neutral' {
  switch ((status ?? '').toLowerCase()) {
    case 'completed':
    case 'success':
    case 'passed':
      return 'ok';
    case 'deviated':
    case 'rollback_pending':
    // T10: rejected (quorum refused) and expired (approval TTL lapsed) are
    // terminal but not execution failures — the run never started.
    case 'rejected':
    case 'expired':
      return 'warn';
    case 'failed':
    case 'failure':
    case 'aborted':
    case 'orphaned':
      return 'fail';
    // queued / validating / running / stopping / pending_approval → neutral.
    default:
      return 'neutral';
  }
}

/**
 * Run states in which a run can still transition — keep polling. Anything
 * not in this set is treated as terminal, so unknown future terminal states
 * stop the poll loop automatically. `pending_approval` (T10) is active: the
 * run is parked until the quorum is met, the request is rejected, or its TTL
 * lapses.
 */
export const ACTIVE_RUN_STATES: ReadonlySet<string> = new Set([
  'queued',
  'validating',
  'running',
  'stopping',
  'pending_approval'
]);
