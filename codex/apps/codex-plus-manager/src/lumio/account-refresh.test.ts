import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  ACCOUNT_AUTO_REFRESH_MS,
  WINDOW_SHOWN_EVENT,
  WINDOW_SHOWN_REFRESH_MIN_GAP_MS,
  shouldAutoRefresh,
} from "./account-refresh.ts";

describe("shouldAutoRefresh", () => {
  it("从未同步过（cachedAt 为空）要立即刷新", () => {
    assert.equal(shouldAutoRefresh(null, Date.now(), WINDOW_SHOWN_REFRESH_MIN_GAP_MS), true);
  });

  it("无法解析的同步时间视为从未同步，要刷新", () => {
    assert.equal(shouldAutoRefresh("not-a-date", Date.now(), WINDOW_SHOWN_REFRESH_MIN_GAP_MS), true);
  });

  it("距上次同步不足最小间隔不重复拉取", () => {
    const now = Date.now();
    const recent = new Date(now - 5_000).toISOString();
    assert.equal(shouldAutoRefresh(recent, now, WINDOW_SHOWN_REFRESH_MIN_GAP_MS), false);
  });

  it("距上次同步超过最小间隔要刷新", () => {
    const now = Date.now();
    const stale = new Date(now - WINDOW_SHOWN_REFRESH_MIN_GAP_MS - 1).toISOString();
    assert.equal(shouldAutoRefresh(stale, now, WINDOW_SHOWN_REFRESH_MIN_GAP_MS), true);
  });

  it("在线定时轮询的周期不短于窗口唤起的最小间隔", () => {
    assert.ok(ACCOUNT_AUTO_REFRESH_MS >= WINDOW_SHOWN_REFRESH_MIN_GAP_MS);
  });
});

describe("WINDOW_SHOWN_EVENT", () => {
  it("与 Rust 侧 show_main_window 的事件名逐字一致", () => {
    assert.equal(WINDOW_SHOWN_EVENT, "lumio://window-shown");
  });
});
