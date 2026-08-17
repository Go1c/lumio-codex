import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { clearSession } from "@lumio/auth";

import { PROFILE, TOKENS, envelope, failure, renderApp, stubFetch } from "@/test/utils";

afterEach(() => {
  clearSession();
  vi.unstubAllGlobals();
});

async function submitLogin() {
  await userEvent.type(await screen.findByLabelText("邮箱"), "user@example.com");
  await userEvent.type(screen.getByLabelText("密码"), "supersecret");
  await userEvent.click(screen.getByRole("button", { name: "登录" }));
}

describe("登录页", () => {
  it("交叉文案指向 BestCodex，不再写两个旧产品", async () => {
    stubFetch({ "/auth/login": () => failure(401, "INVALID_CREDENTIALS") });
    renderApp("/login");

    expect((await screen.findAllByText(/BestCodex/)).length).toBeGreaterThan(0);
    expect(screen.queryByText(/一个 Lumio 账号/)).not.toBeInTheDocument();
    expect(screen.queryByText(/用于 BestCodex/)).not.toBeInTheDocument();
    expect(screen.queryByText(/Lumio Codex/)).not.toBeInTheDocument();
    expect(screen.queryByText(/CC避风港/)).not.toBeInTheDocument();
  });

  it("登录页用户可见品牌是 BestCodex，不出现 Lumio 商标", async () => {
    stubFetch({ "/auth/login": () => failure(401, "INVALID_CREDENTIALS") });
    renderApp("/login");

    expect(await screen.findByRole("heading", { name: "登录" })).toBeInTheDocument();
    expect(document.querySelector(".site-header .logo")?.textContent).toMatch(/BestCodex/);
    expect(screen.queryByRole("link", { name: /^Lumio$/ })).not.toBeInTheDocument();
    expect(screen.queryAllByText(/Lumio/)).toHaveLength(0);
    expect(document.querySelector('img[src="/bestcodex-icon.jpg"]')).not.toBeNull();
  });

  it("凭据错误展示统一文案", async () => {
    stubFetch({ "/auth/login": () => failure(401, "INVALID_CREDENTIALS") });

    renderApp("/login");
    await submitLogin();

    expect(await screen.findByText("邮箱或密码不正确。")).toBeInTheDocument();
  });

  it("2FA 挑战也是 200 响应，进入两步验证再完成登录", async () => {
    stubFetch({
      "/auth/login/2fa": () => envelope(TOKENS),
      "/auth/login": () =>
        envelope({
          requires_2fa: true,
          temp_token: "tmp_123",
          user_email_masked: "u***@example.com",
        }),
      "/auth/me": () => envelope(PROFILE),
    });

    renderApp("/login");
    await submitLogin();

    expect(await screen.findByText(/u\*\*\*@example.com/)).toBeInTheDocument();
    await userEvent.type(screen.getByLabelText("两步验证码"), "654321");
    await userEvent.click(screen.getByRole("button", { name: "验证并登录" }));

    expect(await screen.findByRole("heading", { name: "账户中心" })).toBeInTheDocument();
  });

  it("两步验证码错误时留在当前步骤并提示", async () => {
    stubFetch({
      "/auth/login/2fa": () => failure(400, "TOTP_INVALID_CODE"),
      "/auth/login": () =>
        envelope({
          requires_2fa: true,
          temp_token: "tmp_123",
          user_email_masked: "u***@example.com",
        }),
    });

    renderApp("/login");
    await submitLogin();
    await userEvent.type(await screen.findByLabelText("两步验证码"), "000000");
    await userEvent.click(screen.getByRole("button", { name: "验证并登录" }));

    expect(await screen.findByText("两步验证码不正确，请重新输入。")).toBeInTheDocument();
    expect(screen.getByLabelText("两步验证码")).toBeInTheDocument();
  });

  it("带 next 的站内回跳在登录后生效", async () => {
    stubFetch({
      "/auth/login": () => envelope(TOKENS),
      "/auth/me": () => envelope(PROFILE),
    });

    renderApp("/login?next=%2Faccount");
    await submitLogin();

    expect(await screen.findByRole("heading", { name: "账户中心" })).toBeInTheDocument();
  });

  it("注册入口保留 next，跨页不丢回跳目标", async () => {
    stubFetch({ "/auth/login": () => failure(401, "INVALID_CREDENTIALS") });

    renderApp("/login?next=https%3A%2F%2Fcc.bestcodex.app%2Fpricing");

    expect(await screen.findByRole("link", { name: "创建账号" })).toHaveAttribute(
      "href",
      "/signup?next=https%3A%2F%2Fcc.bestcodex.app%2Fpricing",
    );
  });
});
