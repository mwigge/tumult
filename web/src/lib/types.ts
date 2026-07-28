// Shared types mirroring the kronika-api JSON shapes.

export interface SparkPoint {
  ts: number;
  v: number;
}

export interface Kpi {
  name: string;
  label: string;
  unit: 'count' | 'ratio' | 'seconds';
  value: number | null;
  delta: number | null;
  spark: SparkPoint[];
}

export interface Overview {
  range: string;
  from_ns: number;
  to_ns: number;
  kpis: Kpi[];
  experiments_per_day: SparkPoint[];
  targets: { target: string; experiments: number; pass_rate: number | null }[];
  faults: { fault_type: string; fault_subtype: string | null; count: number }[];
}

export interface ExperimentRow {
  id: string;
  name: string | null;
  started_ns: number;
  duration_ns: number | null;
  trace_id: string;
  target_system: string | null;
  target_technology: string | null;
  target_environment: string | null;
  status: string | null;
  deviations: string | null;
  duration_ms: string | null;
  faults: string | null;
}

export interface Span {
  ts_ns: number;
  trace_id: string;
  span_id: string;
  parent_span_id: string | null;
  span_name: string;
  span_kind: string;
  duration_ns: number;
  status_code: string;
  status_message: string;
  service_name: string;
  fault_type: string | null;
  fault_subtype: string | null;
  // Present on trace-detail rows (experiment roots carry these in tumult).
  experiment_id?: string | null;
  experiment_name?: string | null;
  span_attrs: Record<string, string>;
  events: unknown;
}

export interface LogRow {
  ts_ns: number;
  severity_text: string | null;
  body: string;
  trace_id: string | null;
  span_id: string | null;
  log_attrs: Record<string, string>;
}

export interface MetricPoint {
  kind: 'sum' | 'gauge';
  ts_ns: number;
  metric_name: string;
  value: number;
  outcome_status: string | null;
  plugin_name: string | null;
}

export interface ExperimentDetail {
  experiment: ExperimentRow;
  spans: Span[];
  logs: LogRow[];
  metrics: MetricPoint[];
}

export interface Dimensions {
  outcomes: string[];
  targets: string[];
  faults: string[];
  experiments: string[];
}

export interface MetricDefInfo {
  name: string;
  description: string | null;
}

export interface Timeseries {
  metric: string;
  description: string | null;
  interval: string;
  range: string;
  points: { bucket_s: number; value: number | null }[];
}

export interface AskResponse {
  configured: boolean;
  source?: 'golden' | 'llm';
  sql?: string;
  rows?: Record<string, unknown>[];
  error?: string;
}

export interface ReportFile {
  name: string;
  bytes: number;
  modified_s: number;
}

export interface LogEntry {
  ts_ns: number;
  severity_text: string | null;
  body: string;
  trace_id: string | null;
  span_id: string | null;
  service_name: string | null;
  experiment_id: string | null;
  log_attrs: Record<string, string>;
  resource_attrs: Record<string, string>;
}

export interface LogVolume {
  interval: string;
  bucket_s: number;
  rows: { ts: number; severity: string; count: number }[];
}

export interface TraceRow {
  trace_id: string;
  started_ns: number;
  duration_ns: number;
  span_count: number;
  error_count: number;
  root_name: string | null;
  service_name: string | null;
  experiment_id: string | null;
  experiment_name: string | null;
  status: string | null;
}

export interface TraceDurations {
  points: { trace_id: string; ts_ns: number; duration_ms: number }[];
  p50_ms: number | null;
  p95_ms: number | null;
  p99_ms: number | null;
}

export interface TraceDetail {
  trace_id: string;
  spans: Span[];
  logs: LogRow[];
}

export interface MetricCatalogEntry {
  name: string;
  types: ('sum' | 'gauge' | 'histogram')[];
  dimensions: string[];
}

export interface MetricSeries {
  group: string | null;
  points: { ts: number; v?: number | null; avg?: number | null; p95?: number | null }[];
}

export interface MetricQueryResult {
  name: string;
  type: 'sum' | 'gauge' | 'histogram';
  interval: string;
  range: string;
  group_by: string | null;
  series: MetricSeries[];
}
