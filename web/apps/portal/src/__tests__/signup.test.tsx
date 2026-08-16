import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { clearSession } from "@lumio/auth";

import { PROFILE, TOKENS, envelope, failure, renderApp, stubFetch } from "@/test/utils";

const OPEN_SETTINGS = {
  registration_enabled: true,
  email_verify_enabled: false,
  registration_email_suffix_whitelist: [],
  password_reset_enabled: true,
  login_agreement_enabled: false,
};

afterEach(() => {
  clearSession();
  sessionStorage.clear();
  vi.unstubAllGlobals();
});

describe("注册页尊重 settings/public", () => {
  it("注册关闭时不给表单，只说明原因", async () => {
    stubFetch({
      "/settings/public": () => envelope({ ...OPEN_SETTINGS, registration_enabled: false }),
    });

    renderApp("/signup");

    expect(await screen.findByText(/当前未开放注册/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "创建账号" })).not.toBeInTheDocument();
  });

  it("需要邮箱验证时显示验证码字段，发送后进入倒计时", async () => {
    stubFetch({
      "/settings/public": () => envelope({ ...OPEN_SETTINGS, email_verify_enabled: true }),
      "/auth/send-verify-code": () => envelope({ countdown: 60 }),
    });

    renderApp("/signup");

    const email = await screen.findByLabelText("邮箱");
    await userEvent.type(email, "user@example.com");
    expect(screen.getByLabelText("邮箱验证码")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "发送验证码" }));

    await waitFor(() =>
      expect(screen.getByRole("button", { name: /秒后可重发/ })).toBeDisabled(),
    );
  });

  it("邮箱后缀白名单不通过时本地拦截，不发注册请求", async () => {
    const fetchMock = stubFetch({
      "/settings/public": () =>
        envelope({ ...OPEN_SETTINGS, registration_email_suffix_whitelist: ["@lumio.games"] }),
    });

    renderApp("/signup");

    await userEvent.type(await screen.findByLabelText("邮箱"), "user@example.com");
    await userEvent.type(screen.getByLabelText("密码"), "pw12345678");
    await userEvent.click(screen.getByRole("button", { name: "创建账号" }));

    expect(await screen.findByText("仅支持以下邮箱后缀注册：@lumio.games")).toBeInTheDocument();
    expect(fetchMock.mock.calls.some(([url]) => String(url).includes("/auth/register"))).toBe(
      false,
    );
  });

  it("站点开启协议时必须先勾选才能提交", async () => {
    const fetchMock = stubFetch({
      "/settings/public": () =>
        envelope({
          ...OPEN_SETTINGS,
          login_agreement_enabled: true,
          login_agreement_revision: "r1",
          login_agreement_documents: [{ id: "terms", title: "服务条款", content_md: "# 条款" }],
        }),
    });

    renderApp("/signup");

    await userEvent.type(await screen.findByLabelText("邮箱"), "user@example.com");
    await userEvent.type(screen.getByLabelText("密码"), "pw12345678");
    await userEvent.click(screen.getByRole("button", { name: "创建账号" }));

    expect(await screen.findByText(/请先阅读并同意/)).toBeInTheDocument();
    expect(fetchMock.mock.calls.some(([url]) => String(url).includes("/auth/register"))).toBe(
      false,
    );

    await userEvent.click(screen.getByRole("checkbox"));
    await userEvent.click(screen.getByRole("button", { name: "创建账号" }));

    await waitFor(() =>
      expect(fetchMock.mock.calls.some(([url]) => String(url).includes("/auth/register"))).toBe(
        true,
      ),
    );
  });

  it("注册成功后写入会话并进入账户中心", async () => {
    stubFetch({
      "/settings/public": () => envelope(OPEN_SETTINGS),
      "/auth/register": () => envelope(TOKENS),
      "/auth/me": () => envelope(PROFILE),
    });

    renderApp("/signup");

    await userEvent.type(await screen.findByLabelText("邮箱"), "user@example.com");
    await userEvent.type(screen.getByLabelText("密码"), "pw12345678");
    await userEvent.click(screen.getByRole("button", { name: "创建账号" }));

    expect(await screen.findByRole("heading", { name: "账户中心" })).toBeInTheDocument();
    expect(document.cookie).toContain("lumio_at");
  });

  it("服务端拒绝时展示归一化后的中文文案，不回显服务端原文", async () => {
    stubFetch({
      "/settings/public": () => envelope(OPEN_SETTINGS),
      "/auth/register": () => failure(409, "EMAIL_EXISTS"),
    });

    renderApp("/signup");

    await userEvent.type(await screen.findByLabelText("邮箱"), "user@example.com");
    await userEvent.type(screen.getByLabelText("密码"), "pw12345678");
    await userEvent.click(screen.getByRole("button", { name: "创建账号" }));

    expect(await screen.findByText("该邮箱已注册，请直接登录。")).toBeInTheDocument();
    expect(screen.queryByText(/服务端原文/)).not.toBeInTheDocument();
  });

  it("邀请链接 ?aff= 的归因码随注册提交，页面有邀请提示", async () => {
    const fetchMock = stubFetch({
      "/settings/public": () => envelope(OPEN_SETTINGS),
      "/auth/register": () => envelope(TOKENS),
      "/auth/me": () => envelope(PROFILE),
      "/user/aff": () => envelope({}),
    });

    renderApp("/register?aff=abc123xy");

    expect(await screen.findByText(/已接受好友邀请（ABC123XY）/)).toBeInTheDocument();

    await userEvent.type(await screen.findByLabelText("邮箱"), "user@example.com");
    await userEvent.type(screen.getByLabelText("密码"), "pw12345678");
    await userEvent.click(screen.getByRole("button", { name: "创建账号" }));

    await waitFor(() => {
      const call = fetchMock.mock.calls.find(([url]) => String(url).includes("/auth/register"));
      expect(call).toBeTruthy();
      expect(JSON.parse(String(call?.[1]?.body)).aff_code).toBe("abc123xy");
    });
  });
});
