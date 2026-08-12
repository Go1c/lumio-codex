import { describe, expect, it } from "vitest";

import { redirectTarget } from "@/lib/redirect";

describe("redirectTarget", () => {
  it("站内路径走前端路由，不整页刷新", () => {
    expect(redirectTarget("/account", "/account")).toEqual({ kind: "internal", path: "/account" });
  });

  it("子站地址整页跳回去", () => {
    expect(redirectTarget("https://cc.lumiogame.com/pricing", "/account")).toEqual({
      kind: "external",
      url: "https://cc.lumiogame.com/pricing",
    });
  });

  it("外站地址一律退回默认落点", () => {
    expect(redirectTarget("https://evil.com/steal", "/account")).toEqual({
      kind: "internal",
      path: "/account",
    });
    expect(redirectTarget(null, "/account")).toEqual({ kind: "internal", path: "/account" });
  });
});
