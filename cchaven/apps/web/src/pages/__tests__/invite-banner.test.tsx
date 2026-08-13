import { screen, waitFor } from "@testing-library/react";
import { HttpResponse, http } from "msw";
import { describe, expect, it, vi } from "vitest";

import { db, VALID_INVITE_CODE } from "@/mocks/db";
import { fail } from "@/mocks/handlers";
import { server } from "@/mocks/server";
import { renderApp } from "@/test/utils";

/**
 * 邀请横幅的唯一判据是 `GET /api/v1/invites/current`（服务端读 HttpOnly `cch_ref`）。
 * 前端不得自己缓存归因：那份副本不随 cookie 过期、也不随邀请码停用而失效，
 * 会造成「首页承诺首月免费、注册后拿不到」的错误承诺。
 */

const API = "*/api/v1";

const HOME_PLAIN = "🎁 获朋友邀请？注册并登录 APP，即享首月免费试用（每个账号限一次）。";
const HOME_ACTIVE = "🎁 Alex 的邀请已记录 — 注册并登录 APP，即享首月免费试用。";
const SIGNUP_BANNER = "🎁 Alex 邀请你使用CC避风港 — 注册并登录 APP 即享首月免费试用。";

/** 让 `/invites/current` 一直挂起，用于观察「归因未确定」这一态。 */
function pendingAttribution() {
  server.use(http.get(`${API}/invites/current`, () => new Promise<never>(() => {})));
}

describe("邀请横幅（GET /invites/current）", () => {
  it("attributed:true 时首页高亮并显示邀请人", async () => {
    db.inviteAttributed = true;
    renderApp("/");

    expect(await screen.findByText(HOME_ACTIVE)).toBeInTheDocument();
    expect(screen.queryByText(HOME_PLAIN)).not.toBeInTheDocument();
  });

  it("attributed:true 时注册页显示绿色横幅与邀请人", async () => {
    db.inviteAttributed = true;
    renderApp("/signup");

    expect(await screen.findByText(SIGNUP_BANNER)).toBeInTheDocument();
  });

  it("attributed:false 时首页只保留 4.1 的常驻文案，不高亮", async () => {
    renderApp("/");

    expect(await screen.findByText(HOME_PLAIN)).toBeInTheDocument();
    await waitFor(() => expect(screen.queryByText(HOME_ACTIVE)).not.toBeInTheDocument());
  });

  it("attributed:false 时注册页不渲染横幅", async () => {
    renderApp("/signup");

    expect(await screen.findByRole("heading", { name: "创建账号" })).toBeInTheDocument();
    await waitFor(() => expect(screen.queryByText(SIGNUP_BANNER)).not.toBeInTheDocument());
  });

  it("接口失败时静默降级：不渲染横幅，也不弹错误条", async () => {
    server.use(
      http.get(`${API}/invites/current`, () => fail(500, "internal_error", "服务暂时不可用，请稍后重试。")),
    );
    renderApp("/signup");

    await screen.findByRole("heading", { name: "创建账号" });
    await waitFor(() => expect(screen.queryByText(SIGNUP_BANNER)).not.toBeInTheDocument());
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(screen.queryByText("服务暂时不可用，请稍后重试。")).not.toBeInTheDocument();
  });

  it("网络故障同样静默：首页保持常驻文案且无错误条", async () => {
    server.use(http.get(`${API}/invites/current`, () => HttpResponse.error()));
    renderApp("/");

    expect(await screen.findByText(HOME_PLAIN)).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("归因未确定前不闪横幅", async () => {
    db.inviteAttributed = true;
    pendingAttribution();
    renderApp("/signup");

    await screen.findByRole("heading", { name: "创建账号" });
    expect(screen.queryByText(SIGNUP_BANNER)).not.toBeInTheDocument();
  });

  it("同一次会话内只请求一次，跨页面共用结果且不轮询", async () => {
    db.inviteAttributed = true;
    let calls = 0;
    server.use(
      http.get(`${API}/invites/current`, () => {
        calls += 1;
        return HttpResponse.json({ data: { attributed: true, inviter: "Alex", trial_days: 30 } });
      }),
    );

    const { user } = renderApp("/");
    expect(await screen.findByText(HOME_ACTIVE)).toBeInTheDocument();

    await user.click(screen.getByRole("link", { name: "免费开始" }));
    expect(await screen.findByText(SIGNUP_BANNER)).toBeInTheDocument();

    await new Promise((resolve) => setTimeout(resolve, 300));
    expect(calls).toBe(1);
  });

  it("落地页拿到的归因直接复用，注册页横幅无需再问一次", async () => {
    const { user } = renderApp(`/i/${VALID_INVITE_CODE}`);

    await screen.findByRole("heading", { name: "Alex 邀请你使用CC避风港" });

    let currentCalls = 0;
    server.use(
      http.get(`${API}/invites/current`, () => {
        currentCalls += 1;
        return HttpResponse.json({ data: { attributed: false } });
      }),
    );

    await user.click(screen.getByRole("link", { name: "注册领取首月免费" }));
    expect(await screen.findByText(SIGNUP_BANNER)).toBeInTheDocument();
    expect(currentCalls).toBe(0);
  });

  it("不再有任何 localStorage 读写（防回归）", async () => {
    const getItem = vi.spyOn(window.localStorage, "getItem");
    const setItem = vi.spyOn(window.localStorage, "setItem");
    const removeItem = vi.spyOn(window.localStorage, "removeItem");

    try {
      const { user, unmount } = renderApp(`/i/${VALID_INVITE_CODE}`);
      await screen.findByRole("heading", { name: "Alex 邀请你使用CC避风港" });

      await user.click(screen.getByRole("link", { name: "注册领取首月免费" }));
      await screen.findByText(SIGNUP_BANNER);
      unmount();

      renderApp("/");
      await screen.findByText(HOME_ACTIVE);

      expect(getItem).not.toHaveBeenCalled();
      expect(setItem).not.toHaveBeenCalled();
      expect(removeItem).not.toHaveBeenCalled();
    } finally {
      vi.restoreAllMocks();
    }
  });
});
