import assert from "node:assert/strict";
import test from "node:test";
import { filterEvents } from "../src/features/diagnostics/filter.ts";

function event(overrides = {}) {
  return {
    schemaVersion: "fns-diagnostic-event/1",
    timestamp: "2026-08-10T10:00:00.000Z",
    level: "info",
    component: "transport",
    eventName: "workspace.ack.confirmed",
    message: "ok",
    projectRef: "project-a",
    runId: "run-1",
    traceId: "trace-1",
    connectionGeneration: 1,
    requestId: null,
    operationId: null,
    streamId: null,
    errorCode: null,
    retryable: false,
    fields: {},
    ...overrides,
  };
}

const sample = [
  event({ level: "info", component: "transport", eventName: "workspace.ack.confirmed", runId: "run-1" }),
  event({
    level: "error",
    component: "watcher",
    eventName: "watcher.overflow",
    runId: "run-2",
    timestamp: "2026-08-10T10:01:00.000Z",
  }),
  event({
    level: "warn",
    component: "transport",
    eventName: "transport.reconnect",
    runId: "run-1",
    timestamp: "2026-08-10T10:02:00.000Z",
  }),
];

test("filterEvents returns all when filter empty", () => {
  assert.equal(filterEvents(sample, {}).length, 3);
});

test("filterEvents by level", () => {
  const result = filterEvents(sample, { level: "error" });
  assert.equal(result.length, 1);
  assert.equal(result[0].eventName, "watcher.overflow");
});

test("filterEvents by component", () => {
  const result = filterEvents(sample, { component: "transport" });
  assert.equal(result.length, 2);
});

test("filterEvents by eventName", () => {
  const result = filterEvents(sample, { eventName: "transport.reconnect" });
  assert.equal(result.length, 1);
  assert.equal(result[0].level, "warn");
});

test("filterEvents by runId", () => {
  const result = filterEvents(sample, { runId: "run-1" });
  assert.equal(result.length, 2);
});

test("filterEvents combines predicates with AND", () => {
  const result = filterEvents(sample, {
    component: "transport",
    runId: "run-1",
    level: "warn",
  });
  assert.equal(result.length, 1);
  assert.equal(result[0].eventName, "transport.reconnect");
});
