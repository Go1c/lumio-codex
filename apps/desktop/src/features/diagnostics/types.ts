/** DTOs aligned with contracts/diagnostics schemas (fns-*-v1). */

export const DIAGNOSTIC_EVENT_SCHEMA = "fns-diagnostic-event/1" as const;
export const HEALTH_SNAPSHOT_SCHEMA = "fns-health-snapshot/1" as const;
export const DIAGNOSTIC_RUN_SCHEMA = "fns-diagnostic-run/1" as const;

export type DiagnosticLevel = "trace" | "debug" | "info" | "warn" | "error";

export type ProgressBoundary =
  | "watcher"
  | "outbox"
  | "transport"
  | "server"
  | "stream"
  | "apply"
  | "ack"
  | "ui-false-online"
  | "unknown";

export type DiagnosticRunOutcome =
  | "passed"
  | "failed"
  | "cancelled"
  | "timeout"
  | "crashed";

export interface RedactionSummary {
  secretHits: number;
  pathRedactions: number;
  fieldsRemoved: number;
}

export interface DiagnosticEvent {
  schemaVersion: typeof DIAGNOSTIC_EVENT_SCHEMA;
  timestamp: string;
  level: DiagnosticLevel;
  component: string;
  eventName: string;
  message: string;
  projectRef: string;
  runId: string;
  traceId: string;
  connectionGeneration: number;
  requestId: string | null;
  operationId: string | null;
  streamId: string | null;
  errorCode: string | null;
  retryable: boolean;
  fields: Record<string, unknown>;
}

export interface HealthSnapshot {
  schemaVersion: typeof HEALTH_SNAPSHOT_SCHEMA;
  timestamp: string;
  runId: string;
  projectRef: string;
  connectionGeneration: number;
  lastProgressBoundary: ProgressBoundary;
  desktop: Record<string, unknown>;
  process: Record<string, unknown>;
  watcher: Record<string, unknown>;
  outbox: Record<string, unknown>;
  transport: Record<string, unknown>;
  stream: Record<string, unknown>;
  cursor: Record<string, unknown>;
  server: Record<string, unknown>;
}

export interface DiagnosticRun {
  schemaVersion: typeof DIAGNOSTIC_RUN_SCHEMA;
  runId: string;
  startedAt: string;
  finishedAt: string | null;
  profile: string;
  outcome: DiagnosticRunOutcome;
  lastPassedBoundary: string | null;
  firstFailedBoundary: string | null;
  scenarioIds: string[];
  eventIds: string[];
  artifactPaths: string[];
  redactionSummary: RedactionSummary;
}

export interface SupportBundleTimeRange {
  from: string | null;
  to: string | null;
}

export interface SupportBundlePreview {
  eventCount: number;
  timeRange: SupportBundleTimeRange;
  redactionSummary: RedactionSummary;
  includesPaths: boolean;
}

export interface SupportBundleExportResult {
  path: string;
  redactionSummary: RedactionSummary;
}

export interface SelfTestStartResult {
  runId: string;
  outcome?: string;
  manifestPath?: string;
  bugPackagePath?: string;
}

/** Filters for listEvents. projectId is always required; events never cross projects. */
export interface EventFilter {
  projectId: string;
  level?: DiagnosticLevel | DiagnosticLevel[];
  component?: string;
  eventName?: string;
  runId?: string;
  limit?: number;
}

export interface TimelineFilter {
  level?: DiagnosticLevel | "";
  component?: string;
  eventName?: string;
  runId?: string;
}
