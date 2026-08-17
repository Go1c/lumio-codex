import { screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { clearSession } from "@lumio/auth";
import { siteUrl } from "@lumio/ui";

import { renderApp, stubFetch } from "@/test/utils";

const navigation = vi.hoisted(() => ({ urls: [] as string[] }));
vi.mock("@/lib/redirect", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/redirect")>();
  return { ...actual, goExternal: (url: string) => void navigation.urls.push(url) };
});

afterEach(() => {
  navigation.urls.length = 0;
  clearSession();
  vi.unstubAllGlobals();
});

describe("门户产品路径", () => {
  it("/codex 不渲染门户 404，而是指向产品站 /codex", () => {
    stubFetch({});
    renderApp("/codex");

    expect(screen.queryByRole("heading", { name: "页面不存在" })).not.toBeInTheDocument();
    expect(screen.getByRole("link", { name: "前往 Codex" })).toHaveAttribute(
      "href",
      "https://bestcodex.app/codex",
    );
    expect(navigation.urls).toEqual(["https://bestcodex.app/codex"]);
  });

  it("/claude 不渲染门户 404，而是指向产品站 /claude", () => {
    stubFetch({});
    renderApp("/claude");

    expect(screen.queryByRole("heading", { name: "页面不存在" })).not.toBeInTheDocument();
    expect(screen.getByRole("link", { name: "前往 Claude" })).toHaveAttribute(
      "href",
      "https://bestcodex.app/claude",
    );
    expect(navigation.urls).toEqual(["https://bestcodex.app/claude"]);
  });
});

describe("门户首页交叉文案", () => {
  it("品牌是 BestCodex，产品卡指向产品站，不再写两个旧产品", () => {
    stubFetch({});
    renderApp("/");

    expect(document.querySelector(".site-header .logo")?.textContent).toMatch(/BestCodex/);
    expect(screen.queryByRole("link", { name: /^Lumio$/ })).not.toBeInTheDocument();
    expect(screen.queryByText("Lumio Codex")).not.toBeInTheDocument();
    expect(screen.queryByText("CC避风港")).not.toBeInTheDocument();
    expect(screen.getAllByText(/一个启动器/).length).toBeGreaterThan(0);
    expect(screen.getAllByText(/一次下载/).length).toBeGreaterThan(0);
    expect(
      screen.getAllByRole("link", { name: /BestCodex/ }).some(
        (link) => link.getAttribute("href") === siteUrl("codex"),
      ),
    ).toBe(true);
    expect(document.querySelector(".product-panel svg")).toBeNull();
    expect(document.querySelector(".panel-mark")).toBeNull();
  });
});
