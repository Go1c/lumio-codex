import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const typesPath = path.join(root, "src/features/diagnostics/types.ts");
const apiPath = path.join(root, "src/lib/diagnosticsApi.ts");

test("schemaVersion string constants match contracts", async () => {
  const types = await readFile(typesPath, "utf8");
  assert.match(types, /fns-diagnostic-event\/1/);
  assert.match(types, /fns-health-snapshot\/1/);
  assert.match(types, /fns-diagnostic-run\/1/);
  assert.match(types, /export interface DiagnosticEvent/);
  assert.match(types, /export interface HealthSnapshot/);
  assert.match(types, /export interface DiagnosticRun/);
  assert.match(types, /export interface SupportBundlePreview/);
  assert.match(types, /lastProgressBoundary/);
  assert.match(types, /redactionSummary/);
  assert.match(types, /includesPaths/);
  assert.match(types, /eventCount/);
  assert.match(types, /timeRange/);
});

test("DiagnosticEvent required fields are present on the type", async () => {
  const types = await readFile(typesPath, "utf8");
  for (const field of [
    "schemaVersion",
    "timestamp",
    "level",
    "component",
    "eventName",
    "message",
    "projectRef",
    "runId",
    "traceId",
    "connectionGeneration",
    "requestId",
    "operationId",
    "streamId",
    "errorCode",
    "retryable",
    "fields",
  ]) {
    assert.match(types, new RegExp(`\\b${field}\\b`), `missing field ${field}`);
  }
});

test("HealthSnapshot boundary sections are present", async () => {
  const types = await readFile(typesPath, "utf8");
  for (const section of [
    "desktop",
    "process",
    "watcher",
    "outbox",
    "transport",
    "stream",
    "cursor",
    "server",
  ]) {
    assert.match(types, new RegExp(`\\b${section}\\b`), `missing section ${section}`);
  }
});

test("facade re-exports schema constants used by memory client", async () => {
  const api = await readFile(apiPath, "utf8");
  assert.match(api, /HEALTH_SNAPSHOT_SCHEMA/);
  assert.match(api, /DIAGNOSTIC_EVENT_SCHEMA|from "\.\.\/features\/diagnostics\/types"/);
});
