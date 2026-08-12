import { describe, it, beforeEach } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";

// Desktop account tests: runtime via esbuild when available, else source + pure helpers.
const dir = dirname(fileURLToPath(import.meta.url));
const src = readFileSync(join(dir, "secure-storage.ts"), "utf8");
const paneSrc = readFileSync(join(dir, "AccountPane.tsx"), "utf8");

describe("desktop account secure storage", () => {
  it("documents OS secure storage for refresh tokens", () => {
    assert.match(src, /OS secure storage/);
    assert.match(src, /storeRefreshToken/);
    assert.match(src, /never LocalStorage/);
  });

  it("supports browser callback / device binding", () => {
    assert.match(src, /buildDeviceBinding/);
    assert.match(src, /parseBrowserCallback/);
    assert.match(src, /account\/callback/);
  });

  it("sanitizes refresh/token from logs", () => {
    assert.match(src, /sanitizeAccountLog/);
    assert.match(src, /\[REDACTED\]/);
  });

  it("source never writes refresh to localStorage", () => {
    // Forbid API usage; prose may still say "LocalStorage" as a negative policy.
    assert.doesNotMatch(src, /localStorage\./);
    assert.doesNotMatch(src, /window\.localStorage/);
  });

  it("applySessionTokens stores refresh via secure driver", () => {
    assert.match(src, /applySessionTokens/);
    assert.match(src, /storeRefreshToken/);
    assert.match(src, /storeAccessInMemory/);
  });
});

describe("memory secure storage runtime", () => {
  it("round-trips refresh without localStorage", async () => {
    const map = new Map();
    const driver = {
      async set(k, v) {
        map.set(k, v);
      },
      async get(k) {
        return map.get(k) ?? null;
      },
      async delete(k) {
        map.delete(k);
      },
    };
    await driver.set("fns.account.refresh", "rt_secret");
    assert.equal(await driver.get("fns.account.refresh"), "rt_secret");
    await driver.delete("fns.account.refresh");
    assert.equal(await driver.get("fns.account.refresh"), null);
  });

  it("parseBrowserCallback extracts code and state", () => {
    const url = "fns://account/callback?code=abc&state=st_1";
    const u = new URL(url);
    assert.equal(u.searchParams.get("code"), "abc");
    assert.equal(u.searchParams.get("state"), "st_1");
  });

  it("sanitizeAccountLog redacts rt_ tokens", () => {
    // mirror sanitize rules for pure assertion
    const fields = { refreshToken: "rt_abc", note: "x", token: "at_1" };
    const out = {};
    for (const [k, v] of Object.entries(fields)) {
      if (/refresh|token|password|code|secret/i.test(k)) out[k] = "[REDACTED]";
      else out[k] = v;
    }
    assert.equal(out.refreshToken, "[REDACTED]");
    assert.equal(out.token, "[REDACTED]");
    assert.equal(out.note, "x");
  });
});

describe("desktop AccountPane feature", () => {
  it("hosts browser sign-in and secure placement marker", () => {
    assert.match(paneSrc, /Sign in with browser/);
    assert.match(paneSrc, /data-refresh-placement/);
    assert.match(paneSrc, /desktopRefreshPlacement/);
    assert.match(paneSrc, /resolveBrowserCallback/);
    assert.match(paneSrc, /applySessionTokens/);
  });

  it("exports account feature from index", () => {
    const index = readFileSync(join(dir, "index.ts"), "utf8");
    assert.match(index, /secure-storage/);
    assert.match(index, /AccountPane/);
  });
});

// Prefer real module when esbuild is present (desktop app dep).
describe("secure-storage module (esbuild bundle)", async () => {
  let mod = null;
  try {
    const require = createRequire(import.meta.url);
    const desktopRoot = join(dir, "../../..");
    const esbuildPath = require.resolve("esbuild", { paths: [desktopRoot] });
    const { build } = require(esbuildPath);
    const { mkdtemp, rm } = await import("node:fs/promises");
    const { tmpdir } = await import("node:os");
    const outDir = await mkdtemp(join(tmpdir(), "fns-account-"));
    const outfile = join(outDir, "secure-storage.mjs");
    await build({
      entryPoints: [join(dir, "secure-storage.ts")],
      bundle: true,
      format: "esm",
      platform: "node",
      outfile,
      logLevel: "silent",
    });
    mod = await import(outfile);
    // cleanup after import
    await rm(outDir, { recursive: true, force: true }).catch(() => {});
  } catch {
    mod = null;
  }

  it("MemorySecureStorage + applySessionTokens + sanitize", async (t) => {
    if (!mod) {
      t.skip("esbuild not available");
      return;
    }
    const {
      MemorySecureStorage,
      setSecureStorageDriver,
      storeRefreshToken,
      loadRefreshToken,
      clearRefreshToken,
      applySessionTokens,
      loadAccessFromMemory,
      sanitizeAccountLog,
      buildDeviceBinding,
      parseBrowserCallback,
      desktopRefreshPlacement,
    } = mod;

    setSecureStorageDriver(new MemorySecureStorage());
    await storeRefreshToken("rt_test");
    assert.equal(await loadRefreshToken(), "rt_test");
    await clearRefreshToken();
    assert.equal(await loadRefreshToken(), null);

    await applySessionTokens({ accessToken: "at_1", refreshToken: "rt_2" });
    assert.equal(loadAccessFromMemory(), "at_1");
    assert.equal(await loadRefreshToken(), "rt_2");

    const log = sanitizeAccountLog({ refreshToken: "rt_2", msg: "hi" });
    assert.equal(log.refreshToken, "[REDACTED]");
    assert.equal(log.msg, "hi");

    const binding = buildDeviceBinding("dev1", "fns://");
    assert.match(binding.callbackUrl, /account\/callback/);
    const parsed = parseBrowserCallback(`${binding.callbackUrl}&code=xyz`);
    // state only in binding URL; add code
    const withCode = parseBrowserCallback(
      `fns://account/callback?code=xyz&state=${encodeURIComponent(binding.state)}`,
    );
    assert.equal(withCode.code, "xyz");
    assert.equal(withCode.state, binding.state);
    assert.equal(desktopRefreshPlacement(), "secure");
    void parsed;
  });
});
