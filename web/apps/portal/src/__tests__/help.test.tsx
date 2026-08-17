import { screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { clearSession } from "@lumio/auth";

import { renderApp, stubFetch } from "@/test/utils";

beforeEach(() => {
  clearSession();
});

afterEach(() => {
  clearSession();
  vi.unstubAllGlobals();
});

describe("门户帮助", () => {
  it("规范 URL /help 渲染五篇主题，门户品牌是 BestCodex", async () => {
    stubFetch({});
    renderApp("/help");

    expect(await screen.findByRole("heading", { name: "需要什么帮助？" })).toBeInTheDocument();
    expect(document.querySelector(".site-header .logo")?.textContent).toMatch(/BestCodex/);
    expect(screen.queryByRole("link", { name: /^Lumio$/ })).not.toBeInTheDocument();
    const topics = screen.getByRole("navigation", { name: "帮助主题" });
    expect(topics).toHaveTextContent("安装");
    expect(topics).toHaveTextContent("未签名");
    expect(topics).toHaveTextContent("登录");
    expect(topics).toHaveTextContent("修复");
    expect(topics).toHaveTextContent("Claude 连服务器");
  });
});
