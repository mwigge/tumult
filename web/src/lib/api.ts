// Fetch wrappers for the kronika query API (same origin when embedded in
// kronikad; vite dev proxies /api to a local daemon).

import type {
  AskResponse,
  Dimensions,
  ExperimentDetail,
  ExperimentRow,
  LogEntry,
  LogVolume,
  MetricCatalogEntry,
  MetricDefInfo,
  MetricQueryResult,
  Overview,
  ReportFile,
  ReportMetaV2,
  ReportTemplate,
  Scorecard,
  Timeseries,
  Topology,
  TraceDetail,
  TraceDurations,
  TraceRow
} from './types';

async function get<T>(path: string): Promise<T> {
  const resp = await fetch(path);
  const body = await resp.json().catch(() => ({}));
  if (!resp.ok) {
    throw new Error(body.error ?? `HTTP ${resp.status}`);
  }
  return body as T;
}

export const api = {
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

  dimensions: () => get<Dimensions>('/api/dimensions'),

  metrics: () => get<{ metrics: MetricDefInfo[] }>('/api/metrics'),

  reports: () => get<{ reports: ReportFile[] }>('/api/reports'),

  scores: (range: string) => get<Scorecard>(`/api/scores?range=${range}`),

  reportsV2: () => get<{ reports: ReportMetaV2[] }>('/api/reports/v2'),

  generateReportV2: async (req: {
    type: ReportTemplate;
    period?: string;
    experiment_id?: string;
    framework?: string;
  }): Promise<ReportMetaV2> => {
    const resp = await fetch('/api/reports/v2/generate', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(req)
    });
    const body = await resp.json().catch(() => ({}));
    if (!resp.ok) throw new Error(body.error ?? `HTTP ${resp.status}`);
    return body as ReportMetaV2;
  },

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
    const resp = await fetch('/api/ask', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ question })
    });
    const body = await resp.json().catch(() => ({}));
    if (!resp.ok && !('configured' in body)) {
      throw new Error(body.error ?? `HTTP ${resp.status}`);
    }
    return body as AskResponse;
  }
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
      return 'ok';
    case 'deviated':
      return 'warn';
    case 'failed':
    case 'failure':
      return 'fail';
    default:
      return 'neutral';
  }
}
