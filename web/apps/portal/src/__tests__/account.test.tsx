import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { clearSession, writeSession } from "@lumio/auth";

import { PROFILE, envelope, renderApp, stubFetch } from "@/test/utils";

beforeEach(() => {
  clearSession();
});

afterEach(() => {
  clearSession();
  vi.unstubAllGlobals();
});

describe("账户中心", () => {
  it("未登录时引导去登录，不请求账户接口", async () => {
    const fetchMock = stubFetch({});

    renderApp("/account");

    expect(await screen.findByRole("link", { name: "去登录" })).toHaveAttribute("href", "/login");
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("已登录时展示邮箱、余额与状态，充值跳 Sub2API 收银台", async () => {
    writeSession({ accessToken: "at-1", refreshToken: "rt-1", expiresIn: 3600 });
    stubFetch({ "/auth/me": () => envelope(PROFILE) });

    renderApp("/account");

    expect(await screen.findByText("user@example.com")).toBeInTheDocument();
    expect(screen.getByText("¥12.50")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /充值/ })).toHaveAttribute(
      "href",
      "https://api.lumio.games/purchase",
    );
  });

  it("登出后清掉共享会话 Cookie", async () => {
    writeSession({ accessToken: "at-1", refreshToken: "rt-1", expiresIn: 3600 });
    stubFetch({
      "/auth/me": () => envelope(PROFILE),
      "/auth/logout": () => envelope({}),
    });

    renderApp("/account");
    await userEvent.click(await screen.findByRole("button", { name: "退出登录" }));

    await waitFor(() => expect(document.cookie).not.toContain("lumio_at"));
    expect(await screen.findByRole("link", { name: "去登录" })).toBeInTheDocument();
  });
});
