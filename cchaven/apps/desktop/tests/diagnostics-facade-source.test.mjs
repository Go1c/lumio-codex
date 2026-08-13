import assert from "node:assert/strict";
import { readFile, readdir } from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const apiPath = path.join(root, "src/lib/diagnosticsApi.ts");
const featuresDir = path.join(root, "src/features/diagnostics");

async function readAllFeatureSources() {
  const names = await readdir(featuresDir);
  const files = names.filter((name) => /\.(tsx?|css)$/.test(name));
  const contents = {};
  for (const name of files) {
    contents[name] = await readFile(path.join(featuresDir, name), "utf8");
  }
  return contents;
}

test("diagnosticsApi facade exists with required client factories", async () => {
  const source = await readFile(apiPath, "utf8");
  assert.match(source, /export interface DiagnosticsClient/);
  assert.match(source, /listEvents\(filter:\s*EventFilter\)/);
  assert.match(source, /getHealth\(projectId:\s*string\)/);
  assert.match(source, /previewSupportBundle\(projectId:\s*string\)/);
  assert.match(source, /exportSupportBundle\(projectId:\s*string\)/);
  assert.match(source, /runSelfTest\(profile:\s*string\)/);
  assert.match(source, /cancelSelfTest\(runId:\s*string\)/);
  assert.match(source, /export function createInvokeDiagnosticsClient/);
  assert.match(source, /export function createMemoryDiagnosticsClient/);
  assert.match(source, /diagnostics_list_events/);
  assert.match(source, /diagnostics_get_health/);
  assert.match(source, /diagnostics_preview_support_bundle/);
  assert.match(source, /diagnostics_export_support_bundle/);
  assert.match(source, /diagnostics_run_self_test/);
  assert.match(source, /diagnostics_cancel_self_test/);
});

test("diagnostics UI components do not call invoke( directly", async () => {
  const sources = await readAllFeatureSources();
  const uiFiles = Object.entries(sources).filter(([name]) =>
    /\.(tsx|ts)$/.test(name),
  );

  for (const [name, source] of uiFiles) {
    assert.doesNotMatch(
      source,
      /\binvoke\s*[<(]/,
      `${name} must not call invoke( — use DiagnosticsClient facade props`,
    );
    assert.doesNotMatch(
      source,
      /from\s+["']@tauri-apps\/api/,
      `${name} must not import tauri api for diagnostics`,
    );
  }

  assert.ok(sources["DiagnosticsPane.tsx"], "DiagnosticsPane.tsx required");
  assert.ok(sources["TimelineView.tsx"], "TimelineView.tsx required");
  assert.ok(sources["HealthView.tsx"], "HealthView.tsx required");
  assert.ok(sources["SelfTestView.tsx"], "SelfTestView.tsx required");
  assert.ok(sources["SupportBundleView.tsx"], "SupportBundleView.tsx required");
  assert.ok(sources["index.ts"], "index.ts barrel required");
});

test("DiagnosticsPane wires tabs and uses client facade methods", async () => {
  const pane = await readFile(
    path.join(featuresDir, "DiagnosticsPane.tsx"),
    "utf8",
  );
  assert.match(pane, /logs\.tabTimeline/);
  assert.match(pane, /logs\.tabHealth/);
  assert.match(pane, /logs\.tabSelfTest/);
  assert.match(pane, /logs\.tabSupportBundle/);
  assert.match(pane, /client\.listEvents/);
  assert.match(pane, /client\.getHealth/);
  assert.match(pane, /DiagnosticsClient/);
});

test("DiagnosticsPane polls only while mounted and invalidates stale refreshes", async () => {
  const pane = await readFile(
    path.join(featuresDir, "DiagnosticsPane.tsx"),
    "utf8",
  );
  assert.match(pane, /DIAGNOSTICS_REFRESH_INTERVAL_MS\s*=\s*5_000/);
  assert.match(pane, /refreshInFlight/);
  assert.match(pane, /requestGeneration/);
  assert.match(pane, /window\.setTimeout/);
  assert.match(pane, /window\.clearTimeout/);
  assert.match(pane, /mounted\.current/);
  assert.match(pane, /logs\.refreshing/);
});

test("SupportBundleView requires preview before export", async () => {
  const source = await readFile(
    path.join(featuresDir, "SupportBundleView.tsx"),
    "utf8",
  );
  assert.match(source, /previewSupportBundle/);
  assert.match(source, /exportSupportBundle/);
  assert.match(source, /disabled=\{busy \|\| !preview\}/);
  assert.match(source, /redactionSummary/);
});

test("TimelineView receives events as props and uses filterEvents", async () => {
  const source = await readFile(
    path.join(featuresDir, "TimelineView.tsx"),
    "utf8",
  );
  assert.match(source, /events:\s*readonly DiagnosticEvent\[\]/);
  assert.match(source, /filterEvents/);
  assert.doesNotMatch(source, /DiagnosticsClient/);
});
