import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import test, { after } from "node:test";
import { build } from "esbuild";

const testsDirectory = dirname(fileURLToPath(import.meta.url));
const desktopDirectory = dirname(testsDirectory);
const outputDirectory = await mkdtemp(
  join(desktopDirectory, ".fns-diagnostics-render-"),
);
const outputFile = join(outputDirectory, "diagnostics-render.mjs");

await build({
  stdin: {
    contents: `
      import { createElement } from "react";
      import { renderToStaticMarkup } from "react-dom/server";
      import TimelineView from "./src/features/diagnostics/TimelineView.tsx";

      export function renderTimeline(props) {
        return renderToStaticMarkup(createElement(TimelineView, props));
      }
    `,
    loader: "tsx",
    resolveDir: desktopDirectory,
  },
  bundle: true,
  format: "esm",
  platform: "node",
  packages: "external",
  outfile: outputFile,
  logLevel: "silent",
});

const { renderTimeline } = await import(pathToFileURL(outputFile).href);

after(async () => {
  await rm(outputDirectory, { recursive: true, force: true });
});

function event(fields = {}) {
  return {
    schemaVersion: "fns-diagnostic-event/1",
    timestamp: "2026-08-10T10:00:00.000Z",
    level: "info",
    component: "transport",
    eventName: "transport.reconnect",
    message: "retrying connection",
    projectRef: "project-a",
    runId: "run-1",
    traceId: "trace-1",
    connectionGeneration: 2,
    requestId: null,
    operationId: null,
    streamId: null,
    errorCode: null,
    retryable: true,
    fields,
  };
}

test("timeline distinguishes initial loading from a verified empty result", () => {
  const loading = renderTimeline({ events: [], loading: true, hasLoaded: false });
  assert.match(loading, /正在读取诊断事件/);
  assert.doesNotMatch(loading, /还没有记录到诊断事件/);

  const empty = renderTimeline({ events: [], loading: false, hasLoaded: true });
  assert.match(empty, /还没有记录到诊断事件/);
  assert.doesNotMatch(empty, /正在读取诊断事件/);
});

test("timeline exposes structured event fields without expanding them by default", () => {
  const html = renderTimeline({
    events: [
      event({
        pendingCommands: 3,
        lastAck: 42,
        reconnectAttempt: { count: 2, backoffMs: 1000 },
      }),
    ],
    loading: false,
    hasLoaded: true,
  });

  assert.match(html, /<details class="diagnostics-event-fields">/);
  assert.match(html, /<summary>字段（3）<\/summary>/);
  assert.match(html, /<dt>pendingCommands<\/dt>/);
  assert.match(html, /<code>3<\/code>/);
  assert.match(html, /<dt>lastAck<\/dt>/);
  assert.match(html, /<code>42<\/code>/);
  assert.match(html, /<dt>reconnectAttempt<\/dt>/);
  assert.match(html, /&quot;backoffMs&quot;:1000/);
  assert.doesNotMatch(html, /<details[^>]*\sopen(?:=|>)/);
});
