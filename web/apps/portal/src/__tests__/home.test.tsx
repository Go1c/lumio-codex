import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { clearSession, writeSession } from "@lumio/auth";

import { PROFILE, envelope, renderApp, stubFetch } from "@/test/utils";

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

describe("门户不再渲染营销首页", () => {
  it("/ 不再展示已删除的营销落地页，而是进入账户中心", async () => {
    stubFetch({});
    renderApp("/");

    expect(screen.queryByText("官方原生")).not.toBeInTheDocument();
    expect(screen.queryByText("BestCodex · 账号中心")).not.toBeInTheDocument();
    expect(screen.queryByText("向下滚动 · 了解 BestCodex")).not.toBeInTheDocument();
    expect(await screen.findByRole("heading", { name: "账户中心" })).toBeInTheDocument();
  });

  it("账户中心点顶栏 BestCodex 标不会回到营销落地页", async () => {
    writeSession({ accessToken: "at-1", refreshToken: "rt-1", expiresIn: 3600 });
    stubFetch({ "/auth/me": () => envelope(PROFILE) });
    renderApp("/account");

    expect(await screen.findByText("user@example.com")).toBeInTheDocument();
    const logo = document.querySelector(".site-header .logo");
    expect(logo).toHaveAttribute("href", "/account");
    await userEvent.click(logo!);

    expect(screen.queryByText("官方原生")).not.toBeInTheDocument();
    expect(screen.queryByText("BestCodex · 账号中心")).not.toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "账户中心" })).toBeInTheDocument();
    expect(screen.getByText("user@example.com")).toBeInTheDocument();
  });
});
