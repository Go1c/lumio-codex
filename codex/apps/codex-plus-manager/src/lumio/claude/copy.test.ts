import assert from "node:assert/strict";
import test from "node:test";

import { formatClaudeEntitlementLine, formatLocalCalendarDate } from "./copy.ts";

test("active copy uses a local calendar day, not a raw ISO string", () => {
  const line = formatClaudeEntitlementLine({
    status: "active",
    expiresAt: "2026-09-18T00:00:00Z",
    daysLeft: 30,
  });
  assert.match(line, /已订阅 · 有效期至/);
  assert.match(line, /剩余 30 天/);
  assert.equal(line.includes("2026-09-18T00:00:00Z"), false);
  assert.match(formatLocalCalendarDate("2026-09-18T00:00:00Z"), /年.*月.*日/);
});

test("trialing, expired and none reuse the old CC product copy", () => {
  assert.equal(
    formatClaudeEntitlementLine({ status: "trialing", daysLeft: 7 }),
    "免费试用中 · 剩余 7 天",
  );
  assert.equal(formatClaudeEntitlementLine({ status: "expired" }), "订阅已过期");
  assert.equal(formatClaudeEntitlementLine({ status: "none" }), "未订阅");
});
