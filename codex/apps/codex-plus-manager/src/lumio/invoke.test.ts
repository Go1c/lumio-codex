import assert from "node:assert/strict";
import test from "node:test";

import {
  LumioCommandError,
  MISSING_PAYLOAD_ERROR_CODE,
  SESSION_EXPIRED_ERROR_CODE,
  isSessionExpired,
  normalizeOptionalAccount,
  readCommandResult,
  readRequiredCommandResult,
} from "./invoke.ts";

import type { LumioAccountSummary } from "./types.ts";

function throwsWithCode(errorCode: string) {
  return (error: unknown) =>
    error instanceof LumioCommandError && error.errorCode === errorCode;
}

test("a successful result hands its payload to the caller", () => {
  assert.deepEqual(
    readRequiredCommandResult({ ok: true, errorCode: null, payload: { step: "verify-account" } }),
    { step: "verify-account" },
  );
});

test("a failed result throws its stable error code", () => {
  assert.throws(
    () =>
      readRequiredCommandResult<{ step: string }>({
        ok: false,
        errorCode: "AUTH_SESSION_EXPIRED",
        payload: null,
      }),
    throwsWithCode("AUTH_SESSION_EXPIRED"),
  );
});

test("a failed result without a code still throws a code the ui can render", () => {
  assert.throws(
    () =>
      readRequiredCommandResult<{ step: string }>({ ok: false, errorCode: null, payload: null }),
    throwsWithCode("UNKNOWN"),
  );
});

test("a successful result missing its payload throws instead of leaking an empty value", () => {
  for (const payload of [null, undefined]) {
    assert.throws(
      () =>
        readRequiredCommandResult<{ step: string }>({
          ok: true,
          errorCode: null,
          payload: payload as { step: string } | null,
        }),
      throwsWithCode(MISSING_PAYLOAD_ERROR_CODE),
    );
  }
});

test("commands whose payload is legitimately empty read the absence as a value", () => {
  assert.equal(readCommandResult({ ok: true, errorCode: null, payload: null }), null);
  assert.equal(
    readCommandResult({
      ok: true,
      errorCode: null,
      payload: undefined as { path: string } | null | undefined,
    }),
    null,
  );
});

test("session expiry is classified from the error code, not from each call site", () => {
  assert.equal(isSessionExpired(new LumioCommandError(SESSION_EXPIRED_ERROR_CODE)), true);
  assert.equal(isSessionExpired(new LumioCommandError("AUTH_INVALID_CREDENTIALS")), false);
  assert.equal(isSessionExpired(new Error(SESSION_EXPIRED_ERROR_CODE)), false);
  assert.equal(isSessionExpired(null), false);
});

test("missing account fields normalize to null so undefined !== null cannot fake a login", () => {
  // Tauri/serde 旧包漏字段时 JS 侧是 undefined；直接 `!== null` 会把它当成「有账户」推进首页并黑屏。
  assert.equal(normalizeOptionalAccount(undefined), null);
  assert.equal(normalizeOptionalAccount(null), null);

  const account: LumioAccountSummary = { email: "a@b.c", balance: 1.5, planLabel: null };
  assert.equal(normalizeOptionalAccount(account), account);
});

test("the nullable reader still refuses to swallow the failure branch", () => {
  assert.throws(
    () => readCommandResult<{ path: string }>({ ok: false, errorCode: "CODEX_APP_INVALID", payload: null }),
    throwsWithCode("CODEX_APP_INVALID"),
  );
});
