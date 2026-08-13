import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { pathToFileURL } from "node:url";
import { build } from "esbuild";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { mkdtemp, rm } from "node:fs/promises";

const desktopDirectory = dirname(dirname(fileURLToPath(import.meta.url)));
const terminalSource = await readFile(
  new URL("../src/components/TerminalPane.tsx", import.meta.url),
  "utf8",
);
const clipboardSource = await readFile(
  new URL("../src/lib/terminalClipboard.ts", import.meta.url),
  "utf8",
);

// --- Source contract: Terminal must wire the clipboard bridge ---------------

test("Terminal enables Option-drag local selection under remote mouse mode", () => {
  assert.match(terminalSource, /macOptionClickForcesSelection:\s*true/);
});

test("Terminal handles Cmd/Ctrl copy and paste shortcuts", () => {
  assert.match(terminalSource, /const key = event\.key\.toLowerCase\(\)/);
  assert.match(terminalSource, /event\.metaKey[^\n]*key === "c"/);
  assert.match(terminalSource, /writeLocalClipboard/);
  assert.match(terminalSource, /readLocalClipboard/);
  assert.match(terminalSource, /pasteFromLocal|Cmd\+V/);
});

test("Terminal strips OSC 52 from PTY output via Osc52Filter", () => {
  assert.match(terminalSource, /Osc52Filter/);
  assert.match(terminalSource, /oscFilter\.push/);
  assert.match(terminalSource, /terminalClipboard/);
});

test("Terminal exposes a Copy toolbar action", () => {
  // The toolbar copy action is Chinese like the rest of the shell.
  assert.match(terminalSource, /t\("terminal\.copySelection"\)/);
  assert.match(terminalSource, /copySelectionClick|getSelection\(\)/);
});

test("clipboard helper module exports Osc52Filter and local clipboard IO", () => {
  assert.match(clipboardSource, /export class Osc52Filter/);
  assert.match(clipboardSource, /export async function writeLocalClipboard/);
  assert.match(clipboardSource, /export async function readLocalClipboard/);
  assert.match(clipboardSource, /export function decodeOsc52Body/);
});

// --- Runtime: Osc52Filter behaviour ----------------------------------------

const outputDirectory = await mkdtemp(join(desktopDirectory, ".fns-clip-test-"));
const outputFile = join(outputDirectory, "terminalClipboard.mjs");

// Provide a minimal atob for the Node test runtime (esbuild target node).
await build({
  entryPoints: [join(desktopDirectory, "src/lib/terminalClipboard.ts")],
  bundle: true,
  format: "esm",
  platform: "node",
  outfile: outputFile,
  logLevel: "silent",
  banner: {
    js: `
      import { Buffer as __Buf } from "node:buffer";
      if (typeof globalThis.atob !== "function") {
        globalThis.atob = (s) => __Buf.from(s, "base64").toString("binary");
      }
      if (typeof globalThis.btoa !== "function") {
        globalThis.btoa = (s) => __Buf.from(s, "binary").toString("base64");
      }
    `,
  },
});

const { Osc52Filter, decodeOsc52Body } = await import(
  pathToFileURL(outputFile).href
);

test("decodeOsc52Body decodes base64 UTF-8 payloads", () => {
  const payload = "https://claude.com/oauth?x=1";
  const b64 = Buffer.from(payload, "utf8").toString("base64");
  assert.equal(decodeOsc52Body(`52;c;${b64}`), payload);
});

test("decodeOsc52Body rejects query-only and empty payloads", () => {
  assert.equal(decodeOsc52Body("52;c;?"), null);
  assert.equal(decodeOsc52Body("52;c;"), null);
  assert.equal(decodeOsc52Body("10;title"), null);
});

test("Osc52Filter strips a complete BEL-terminated OSC 52 sequence", () => {
  const payload = "hello-from-remote";
  const b64 = Buffer.from(payload, "utf8").toString("base64");
  const filter = new Osc52Filter();
  const { display, copies } = filter.push(
    `before\x1b]52;c;${b64}\x07after`,
  );
  assert.equal(display, "beforeafter");
  assert.deepEqual(copies, [payload]);
});

test("Osc52Filter strips a ST-terminated OSC 52 sequence", () => {
  const payload = "st-term";
  const b64 = Buffer.from(payload, "utf8").toString("base64");
  const filter = new Osc52Filter();
  const { display, copies } = filter.push(
    `\x1b]52;c;${b64}\x1b\\tail`,
  );
  assert.equal(display, "tail");
  assert.deepEqual(copies, [payload]);
});

test("Osc52Filter handles sequences split across chunks", () => {
  const payload = "chunked-url";
  const b64 = Buffer.from(payload, "utf8").toString("base64");
  const full = `\x1b]52;c;${b64}\x07`;
  const mid = Math.floor(full.length / 2);
  const filter = new Osc52Filter();
  const a = filter.push("pre" + full.slice(0, mid));
  const b = filter.push(full.slice(mid) + "post");
  assert.equal(a.copies.length, 0);
  assert.deepEqual(b.copies, [payload]);
  assert.equal(a.display + b.display, "prepost");
});

test("Osc52Filter passes non-52 OSC sequences through", () => {
  const filter = new Osc52Filter();
  const seq = "\x1b]0;window-title\x07";
  const { display, copies } = filter.push(`x${seq}y`);
  assert.equal(display, `x${seq}y`);
  assert.deepEqual(copies, []);
});

test("Osc52Filter flush returns held incomplete sequence", () => {
  const filter = new Osc52Filter();
  const partial = filter.push("hi\x1b]52;c;YWJj");
  assert.equal(partial.display, "hi");
  assert.equal(partial.copies.length, 0);
  assert.equal(filter.flush(), "\x1b]52;c;YWJj");
});

// Cleanup bundled temp after tests register (node:test runs body first).
test("cleanup clipboard test bundle", async () => {
  await rm(outputDirectory, { recursive: true, force: true });
});
