import assert from "node:assert/strict";
import test from "node:test";
import {
  createMemoryDiagnosticsClient,
  createInvokeDiagnosticsClient,
} from "../src/lib/diagnosticsApi.ts";

function event(projectRef, overrides = {}) {
  return {
    schemaVersion: "fns-diagnostic-event/1",
    timestamp: "2026-08-10T10:00:00.000Z",
    level: "info",
    component: "transport",
    eventName: "workspace.ack.confirmed",
    message: `event for ${projectRef}`,
    projectRef,
    runId: "run-shared",
    traceId: `trace-${projectRef}`,
    connectionGeneration: 1,
    requestId: null,
    operationId: null,
    streamId: null,
    errorCode: null,
    retryable: false,
    fields: { project: projectRef },
    ...overrides,
  };
}

test("memory listEvents never mixes projects", async () => {
  const client = createMemoryDiagnosticsClient({
    events: [
      event("project-a", { timestamp: "2026-08-10T10:00:01.000Z" }),
      event("project-b", { timestamp: "2026-08-10T10:00:02.000Z" }),
      event("project-a", {
        timestamp: "2026-08-10T10:00:03.000Z",
        eventName: "transport.reconnect",
      }),
    ],
  });

  const forA = await client.listEvents({ projectId: "project-a" });
  assert.equal(forA.length, 2);
  assert.ok(forA.every((e) => e.projectRef === "project-a"));

  const forB = await client.listEvents({ projectId: "project-b" });
  assert.equal(forB.length, 1);
  assert.equal(forB[0].projectRef, "project-b");
});

test("memory listEvents applies optional filters within project", async () => {
  const client = createMemoryDiagnosticsClient({
    events: [
      event("project-a", { level: "info", component: "transport" }),
      event("project-a", {
        level: "error",
        component: "watcher",
        eventName: "watcher.overflow",
        runId: "run-err",
      }),
      event("project-b", { level: "error", component: "watcher" }),
    ],
  });

  const errors = await client.listEvents({
    projectId: "project-a",
    level: "error",
  });
  assert.equal(errors.length, 1);
  assert.equal(errors[0].component, "watcher");

  const byRun = await client.listEvents({
    projectId: "project-a",
    runId: "run-err",
  });
  assert.equal(byRun.length, 1);
});

test("memory getHealth is project scoped", async () => {
  const client = createMemoryDiagnosticsClient({
    healthByProject: {
      "project-a": {
        schemaVersion: "fns-health-snapshot/1",
        timestamp: "2026-08-10T10:21:00.000Z",
        runId: "run-a",
        projectRef: "project-a",
        connectionGeneration: 2,
        lastProgressBoundary: "ack",
        desktop: { appVersion: "0.1.0" },
        process: {},
        watcher: {},
        outbox: {},
        transport: {},
        stream: {},
        cursor: {},
        server: {},
      },
    },
  });

  const a = await client.getHealth("project-a");
  assert.equal(a.lastProgressBoundary, "ack");
  assert.equal(a.projectRef, "project-a");

  const b = await client.getHealth("project-b");
  assert.equal(b.projectRef, "project-b");
  assert.equal(b.lastProgressBoundary, "unknown");
});

test("memory support bundle preview/export stay project scoped", async () => {
  const client = createMemoryDiagnosticsClient({
    events: [event("project-a"), event("project-a"), event("project-b")],
    supportBundleByProject: {
      "project-a": {
        preview: {
          eventCount: 2,
          timeRange: {
            from: "2026-08-10T10:00:00.000Z",
            to: "2026-08-10T10:05:00.000Z",
          },
          redactionSummary: {
            secretHits: 0,
            pathRedactions: 1,
            fieldsRemoved: 0,
          },
          includesPaths: false,
        },
        exportPath: "/tmp/bundle-a.zip",
      },
    },
  });

  const preview = await client.previewSupportBundle("project-a");
  assert.equal(preview.eventCount, 2);
  assert.equal(preview.redactionSummary.pathRedactions, 1);

  const exported = await client.exportSupportBundle("project-a");
  assert.equal(exported.path, "/tmp/bundle-a.zip");
  assert.equal(exported.redactionSummary.pathRedactions, 1);
});

test("memory self test run and cancel", async () => {
  const client = createMemoryDiagnosticsClient();
  const started = await client.runSelfTest("ci-isolation");
  assert.ok(started.runId);
  await client.cancelSelfTest(started.runId);
});

test("invoke facade maps methods to diagnostics_* commands", async () => {
  const calls = [];
  const invokeFn = async (cmd, args) => {
    calls.push({ cmd, args });
    if (cmd === "diagnostics_list_events") return [];
    if (cmd === "diagnostics_get_health") {
      return { lastProgressBoundary: "unknown", projectRef: args.projectId };
    }
    if (cmd === "diagnostics_preview_support_bundle") {
      return {
        eventCount: 0,
        timeRange: { from: null, to: null },
        redactionSummary: { secretHits: 0, pathRedactions: 0, fieldsRemoved: 0 },
        includesPaths: false,
      };
    }
    if (cmd === "diagnostics_export_support_bundle") {
      return {
        path: "/tmp/x.zip",
        redactionSummary: { secretHits: 0, pathRedactions: 0, fieldsRemoved: 0 },
      };
    }
    if (cmd === "diagnostics_run_self_test") return { runId: "r1" };
    if (cmd === "diagnostics_cancel_self_test") return undefined;
    throw new Error(`unexpected ${cmd}`);
  };

  const client = createInvokeDiagnosticsClient(invokeFn);
  await client.listEvents({ projectId: "p1" });
  await client.getHealth("p1");
  await client.previewSupportBundle("p1");
  await client.exportSupportBundle("p1");
  await client.runSelfTest("ci-isolation");
  await client.cancelSelfTest("r1");

  assert.deepEqual(
    calls.map((c) => c.cmd),
    [
      "diagnostics_list_events",
      "diagnostics_get_health",
      "diagnostics_preview_support_bundle",
      "diagnostics_export_support_bundle",
      "diagnostics_run_self_test",
      "diagnostics_cancel_self_test",
    ],
  );
});
