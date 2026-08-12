import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import test, { after } from "node:test";
import { build } from "esbuild";

const testsDirectory = dirname(fileURLToPath(import.meta.url));
const desktopDirectory = dirname(testsDirectory);
const outputDirectory = await mkdtemp(
  join(desktopDirectory, ".fns-conflict-render-"),
);
const outputFile = join(outputDirectory, "conflict-render.mjs");

await build({
  stdin: {
    contents: `
      import { createElement } from "react";
      import { renderToStaticMarkup } from "react-dom/server";
      import { ConflictPaneContent } from "./src/components/ConflictPaneContent.tsx";

      export function renderConflictPane(props) {
        return renderToStaticMarkup(createElement(ConflictPaneContent, {
          ...props,
          onRefresh() {},
          onResolve() {},
        }));
      }

      function textContent(node) {
        if (node === null || node === undefined || typeof node === "boolean") return "";
        if (typeof node === "string" || typeof node === "number") return String(node);
        if (Array.isArray(node)) return node.map(textContent).join("");
        return textContent(node.props?.children);
      }

      function findButton(node, label) {
        if (node === null || node === undefined || typeof node !== "object") return null;
        if (Array.isArray(node)) {
          for (const child of node) {
            const match = findButton(child, label);
            if (match) return match;
          }
          return null;
        }
        if (node.type === "button" && textContent(node) === label) return node;
        return findButton(node.props?.children, label);
      }

      export function clickConflictAction(props, label) {
        const calls = [];
        const tree = ConflictPaneContent({
          ...props,
          onRefresh() {},
          onResolve(conflict, choice) { calls.push({ conflict, choice }); },
        });
        const button = findButton(tree, label);
        if (!button) throw new Error("button not found: " + label);
        button.props.onClick();
        return { disabled: button.props.disabled, calls };
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

const { clickConflictAction, renderConflictPane } = await import(
  pathToFileURL(outputFile).href
);

after(async () => {
  await rm(outputDirectory, { recursive: true, force: true });
});

function props(overrides = {}) {
  return {
    syncRunning: true,
    conflicts: [],
    loading: false,
    hasLoaded: true,
    loadFailure: null,
    actionFailure: null,
    operationsFailure: null,
    operations: [],
    resolving: null,
    receipt: null,
    ...overrides,
  };
}

function side(path, hash) {
  return {
    path,
    pathRevision: "7",
    contentHash: hash,
    size: 12,
    modifiedAtMs: 1_700_000_000_000,
    executable: false,
    tombstone: false,
  };
}

test("a failed initial list never renders the successful no-conflicts claim", () => {
  const html = renderConflictPane(
    props({
      hasLoaded: false,
      loadFailure: "The conflict must be refreshed (conflict_refresh_required)",
    }),
  );

  assert.match(html, /Conflict operation failed/);
  assert.match(html, /Conflict status unavailable/);
  assert.match(html, /conflict_refresh_required/);
  assert.doesNotMatch(html, /No unresolved conflicts/);
  assert.doesNotMatch(html, /Local and remote changes agree/);
});

test("empty success is rendered only after a verified successful list", () => {
  const loading = renderConflictPane(
    props({ hasLoaded: false, loading: true }),
  );
  assert.match(loading, /Loading conflicts/);
  assert.doesNotMatch(loading, /No unresolved conflicts/);

  const verified = renderConflictPane(props());
  assert.match(verified, /No unresolved conflicts/);
  assert.match(verified, /Local and remote changes agree/);
});

test("rename conflicts expose both side paths and the exact merged target", () => {
  const conflict = {
    conflictId: "10000000-0000-4000-8000-000000000031",
    conflictRevision: "9",
    path: "resolved/final-name.txt",
    kind: "rename",
    status: "manual",
    ancestor: side("original/name.txt", "sha256:ancestor"),
    current: side("local/renamed.txt", "sha256:local"),
    incoming: side("remote/renamed.txt", "sha256:remote"),
    createdByOperationId: "10000000-0000-4000-8000-000000000032",
    pendingResolution: null,
    canResolve: true,
    blockedReason: null,
  };

  const html = renderConflictPane(props({ conflicts: [conflict] }));

  assert.match(html, /original\/name\.txt/);
  assert.match(html, /local\/renamed\.txt/);
  assert.match(html, /remote\/renamed\.txt/);
  assert.match(html, /Rename resolution target/);
  assert.match(html, /Resolve with the edited file at resolved\/final-name\.txt/);
  assert.match(html, /Use edited target/);
});

test("late queued receipts and failures remain visible after a project switch", () => {
  const html = renderConflictPane(
    props({
      operations: [
        {
          requestId: "request-late",
          projectGeneration: "old-project-generation",
          conflictId: "conflict-late",
          conflictRevision: "11",
          choice: "incoming",
          phase: "queued",
          receipt: { status: "queued", operationId: "operation-late" },
          error: null,
        },
        {
          requestId: "request-failed",
          projectGeneration: "old-project-generation",
          conflictId: "conflict-failed",
          conflictRevision: "12",
          choice: "current",
          phase: "failed",
          receipt: null,
          error: "conflict_revision_stale",
        },
      ],
    }),
  );

  assert.match(html, /Recent decisions/);
  assert.match(html, /Queued as operation-late/);
  assert.match(html, /Failed: conflict_revision_stale/);
});

test("queued receipts and every non-failure operation phase are visible", () => {
  const html = renderConflictPane(
    props({
      receipt: { status: "queued", operationId: "operation-current" },
      operations: [
        {
          requestId: "request-pending",
          projectGeneration: "project-generation",
          conflictId: "conflict-pending",
          conflictRevision: "20",
          choice: "current",
          phase: "pending",
          receipt: null,
          error: null,
        },
        {
          requestId: "request-dispatched",
          projectGeneration: "project-generation",
          conflictId: "conflict-dispatched",
          conflictRevision: "21",
          choice: "incoming",
          phase: "dispatched",
          receipt: null,
          error: null,
        },
        {
          requestId: "request-cancelled",
          projectGeneration: "project-generation",
          conflictId: "conflict-cancelled",
          conflictRevision: "22",
          choice: "delete",
          phase: "cancelled",
          receipt: null,
          error: "request_cancelled",
        },
      ],
    }),
  );

  assert.match(html, /Resolution queued as/);
  assert.match(html, /operation-current/);
  assert.match(html, /Waiting to send/);
  assert.match(html, /Awaiting agent confirmation/);
  assert.match(html, /Cancelled before sending/);
});

test("independent list, resolution, and history failures are all visible", () => {
  const html = renderConflictPane(
    props({
      actionFailure: "resolution_failed",
      loadFailure: "list_failed",
      operationsFailure: "history_failed",
    }),
  );

  assert.match(html, /Resolution/);
  assert.match(html, /resolution_failed/);
  assert.match(html, /Conflict list/);
  assert.match(html, /list_failed/);
  assert.match(html, /Decision history/);
  assert.match(html, /history_failed/);
});

test("local and remote buttons dispatch their exact durable choices", () => {
  const conflict = {
    conflictId: "10000000-0000-4000-8000-000000000031",
    conflictRevision: "13",
    path: "notes/conflict.txt",
    kind: "content",
    status: "manual",
    ancestor: side("notes/conflict.txt", "sha256:ancestor"),
    current: side("notes/conflict.txt", "sha256:local"),
    incoming: side("notes/conflict.txt", "sha256:remote"),
    createdByOperationId: "10000000-0000-4000-8000-000000000032",
    pendingResolution: null,
    canResolve: true,
    blockedReason: null,
  };

  const local = clickConflictAction(props({ conflicts: [conflict] }), "Keep local");
  assert.equal(local.disabled, false);
  assert.deepEqual(local.calls, [{ conflict, choice: "current" }]);

  const remote = clickConflictAction(props({ conflicts: [conflict] }), "Use remote");
  assert.equal(remote.disabled, false);
  assert.deepEqual(remote.calls, [{ conflict, choice: "incoming" }]);
});
