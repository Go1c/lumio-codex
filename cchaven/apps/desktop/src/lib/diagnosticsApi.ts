import type {
  DiagnosticEvent,
  EventFilter,
  HealthSnapshot,
  SelfTestStartResult,
  SupportBundleExportResult,
  SupportBundlePreview,
} from "../features/diagnostics/types.ts";
import {
  DIAGNOSTIC_EVENT_SCHEMA,
  DIAGNOSTIC_RUN_SCHEMA,
  HEALTH_SNAPSHOT_SCHEMA,
} from "../features/diagnostics/types.ts";

export type { DiagnosticEvent, EventFilter, HealthSnapshot, SupportBundlePreview };
export {
  DIAGNOSTIC_EVENT_SCHEMA,
  DIAGNOSTIC_RUN_SCHEMA,
  HEALTH_SNAPSHOT_SCHEMA,
};

/** Tauri-compatible invoke signature (avoids hard dependency in pure tests). */
export type InvokeFn = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;

export interface DiagnosticsClient {
  listEvents(filter: EventFilter): Promise<DiagnosticEvent[]>;
  getHealth(projectId: string): Promise<HealthSnapshot>;
  previewSupportBundle(projectId: string): Promise<SupportBundlePreview>;
  exportSupportBundle(projectId: string): Promise<SupportBundleExportResult>;
  runSelfTest(profile: string): Promise<SelfTestStartResult>;
  cancelSelfTest(runId: string): Promise<void>;
}

export interface MemoryDiagnosticsSeed {
  events?: DiagnosticEvent[];
  healthByProject?: Record<string, HealthSnapshot>;
  supportBundleByProject?: Record<
    string,
    {
      preview: SupportBundlePreview;
      exportPath?: string;
    }
  >;
}

function matchesEventFilter(
  event: DiagnosticEvent,
  filter: EventFilter,
): boolean {
  if (event.projectRef !== filter.projectId) return false;

  if (filter.level !== undefined) {
    const levels = Array.isArray(filter.level) ? filter.level : [filter.level];
    if (!levels.includes(event.level)) return false;
  }
  if (filter.component && event.component !== filter.component) return false;
  if (filter.eventName && event.eventName !== filter.eventName) return false;
  if (filter.runId && event.runId !== filter.runId) return false;
  return true;
}

/**
 * Typed Tauri facade. Components must call this client — never raw `invoke`
 * for diagnostics commands.
 */
export function createInvokeDiagnosticsClient(
  invokeFn: InvokeFn,
): DiagnosticsClient {
  return {
    listEvents(filter) {
      return invokeFn<DiagnosticEvent[]>("diagnostics_list_events", { filter });
    },
    getHealth(projectId) {
      return invokeFn<HealthSnapshot>("diagnostics_get_health", { projectId });
    },
    previewSupportBundle(projectId) {
      return invokeFn<SupportBundlePreview>("diagnostics_preview_support_bundle", {
        projectId,
      });
    },
    exportSupportBundle(projectId) {
      return invokeFn<SupportBundleExportResult>(
        "diagnostics_export_support_bundle",
        { projectId },
      );
    },
    runSelfTest(profile) {
      return invokeFn<SelfTestStartResult>("diagnostics_run_self_test", {
        profile,
      });
    },
    cancelSelfTest(runId) {
      return invokeFn<void>("diagnostics_cancel_self_test", { runId });
    },
  };
}

function emptyPreview(): SupportBundlePreview {
  return {
    eventCount: 0,
    timeRange: { from: null, to: null },
    redactionSummary: {
      secretHits: 0,
      pathRedactions: 0,
      fieldsRemoved: 0,
    },
    includesPaths: false,
  };
}

function emptyHealth(projectId: string): HealthSnapshot {
  return {
    schemaVersion: HEALTH_SNAPSHOT_SCHEMA,
    timestamp: new Date(0).toISOString(),
    runId: "memory-none",
    projectRef: projectId,
    connectionGeneration: 0,
    lastProgressBoundary: "unknown",
    desktop: {},
    process: {},
    watcher: {},
    outbox: {},
    transport: {},
    stream: {},
    cursor: {},
    server: {},
  };
}

/**
 * In-memory client for tests and UI development without a Tauri backend.
 * Events are always scoped by projectId ↔ projectRef; projects never mix.
 */
export function createMemoryDiagnosticsClient(
  seed: MemoryDiagnosticsSeed = {},
): DiagnosticsClient {
  const events = [...(seed.events ?? [])];
  const healthByProject = { ...(seed.healthByProject ?? {}) };
  const supportBundleByProject = { ...(seed.supportBundleByProject ?? {}) };
  const activeSelfTests = new Map<string, string>();

  return {
    async listEvents(filter) {
      if (!filter.projectId) {
        throw new Error("projectId is required");
      }
      let matched = events.filter((event) => matchesEventFilter(event, filter));
      matched = matched.sort((a, b) => a.timestamp.localeCompare(b.timestamp));
      if (filter.limit !== undefined && filter.limit >= 0) {
        matched = matched.slice(0, filter.limit);
      }
      return matched.map((event) => ({ ...event, fields: { ...event.fields } }));
    },

    async getHealth(projectId) {
      if (!projectId) throw new Error("projectId is required");
      const health = healthByProject[projectId];
      if (!health) return emptyHealth(projectId);
      return { ...health };
    },

    async previewSupportBundle(projectId) {
      if (!projectId) throw new Error("projectId is required");
      const entry = supportBundleByProject[projectId];
      if (entry) return { ...entry.preview, redactionSummary: { ...entry.preview.redactionSummary } };

      const projectEvents = events.filter((e) => e.projectRef === projectId);
      const timestamps = projectEvents.map((e) => e.timestamp).sort();
      return {
        eventCount: projectEvents.length,
        timeRange: {
          from: timestamps[0] ?? null,
          to: timestamps[timestamps.length - 1] ?? null,
        },
        redactionSummary: {
          secretHits: 0,
          pathRedactions: 0,
          fieldsRemoved: 0,
        },
        includesPaths: false,
      };
    },

    async exportSupportBundle(projectId) {
      if (!projectId) throw new Error("projectId is required");
      const entry = supportBundleByProject[projectId];
      const preview = entry?.preview ?? emptyPreview();
      const path =
        entry?.exportPath ??
        `/tmp/fns-support-bundle-${projectId}-${Date.now()}.zip`;
      return {
        path,
        redactionSummary: { ...preview.redactionSummary },
      };
    },

    async runSelfTest(profile) {
      if (!profile.trim()) throw new Error("profile is required");
      if (profile.includes("testOnly=false") || profile.includes('"testOnly": false')) {
        throw new Error("refusing non-testOnly profile");
      }
      const runId = `memory-run-${profile}-${activeSelfTests.size + 1}`;
      activeSelfTests.set(runId, profile);
      return {
        runId,
        outcome: "passed",
        manifestPath: `/memory/selftest/${runId}/diagnostic-run.json`,
        bugPackagePath: `/memory/selftest/${runId}/bug-package.json`,
      };
    },

    async cancelSelfTest(runId) {
      if (!runId) throw new Error("runId is required");
      activeSelfTests.delete(runId);
    },
  };
}
