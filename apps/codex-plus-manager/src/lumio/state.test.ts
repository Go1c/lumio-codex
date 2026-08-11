import assert from "node:assert/strict";
import test from "node:test";

import { initialLumioState, reduceLumioState } from "./state.ts";

test("bootstrap without account enters signed-out", () => {
  const next = reduceLumioState(initialLumioState(), {
    type: "bootstrapped",
    payload: {
      version: "1.0.0",
      platform: "macos",
      arch: "aarch64",
      codexApp: null,
      account: null,
      telemetryEnabled: false,
      autoUpdateEnabled: true,
    },
  });

  assert.equal(next.phase, "signed-out");
  assert.equal(next.telemetryEnabled, false);
});

test("offline readiness never enables payment or refresh", () => {
  const next = reduceLumioState(initialLumioState(), {
    type: "offline-ready",
    cachedAt: "2026-08-11T00:00:00Z",
  });

  assert.equal(next.phase, "ready-offline");
  assert.equal(next.actions.canLaunch, true);
  assert.equal(next.actions.canRefresh, false);
  assert.equal(next.actions.canPay, false);
});

test("bootstrap with an account enters provisioning with actions disabled", () => {
  const next = reduceLumioState(initialLumioState(), {
    type: "bootstrapped",
    payload: {
      version: "1.0.0",
      platform: "windows",
      arch: "x86_64",
      codexApp: null,
      account: {
        email: "user@example.com",
        balance: 12.5,
        planLabel: "Trial",
      },
      telemetryEnabled: true,
      autoUpdateEnabled: false,
    },
  });

  assert.equal(next.phase, "provisioning");
  assert.equal(next.actions.canLaunch, false);
  assert.equal(next.actions.canRefresh, false);
  assert.equal(next.actions.canPay, false);
  assert.equal(next.telemetryEnabled, true);
  assert.equal(next.autoUpdateEnabled, false);
});

test("repair-required preserves no privileged actions", () => {
  const next = reduceLumioState(initialLumioState(), {
    type: "repair-required",
    errorCode: "CODEX_CONFIG_CONFLICT",
  });

  assert.equal(next.phase, "needs-repair");
  assert.equal(next.errorCode, "CODEX_CONFIG_CONFLICT");
  assert.deepEqual(next.actions, {
    canLaunch: false,
    canRefresh: false,
    canPay: false,
  });
});
