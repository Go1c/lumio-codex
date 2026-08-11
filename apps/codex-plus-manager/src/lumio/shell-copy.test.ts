import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { LUMIO_BOOTSTRAP_COMMAND, visibleShellLabels } from "./invoke.ts";

test("shell invokes only the Lumio bootstrap command", () => {
  assert.equal(LUMIO_BOOTSTRAP_COMMAND, "lumio_bootstrap");
});

test("shell label inventory contains only the approved product surface", () => {
  assert.deepEqual(visibleShellLabels, [
    "账户状态",
    "余额与套餐",
    "连接状态",
    "默认模型",
    "充值",
    "启动 Codex",
    "开机启动",
    "自动更新",
    "官方应用路径",
    "遥测",
    "日志导出",
    "配置恢复",
  ]);
});

test("shell copy excludes Codex++ enhancement surfaces", () => {
  const copy = visibleShellLabels.join(" ").toLowerCase();
  for (const forbidden of [
    "provider",
    "base url",
    "api key",
    "stepwise",
    "mcp",
    "plugin",
    "dream skin",
  ]) {
    assert.equal(copy.includes(forbidden), false);
  }
});

test("React entry renders only LumioApp", async () => {
  const main = await readFile(new URL("../main.tsx", import.meta.url), "utf8");

  assert.match(main, /import \{ LumioApp \} from "\.\/LumioApp"/);
  assert.doesNotMatch(main, /from "\.\/App"/);
});
