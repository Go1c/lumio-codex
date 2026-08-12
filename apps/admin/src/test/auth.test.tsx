import { describe, expect, it } from "vitest";
import { screen, waitFor } from "@testing-library/react";
import { http, HttpResponse } from "msw";
import { server } from "../mocks/server";
import { MOCK_CREDENTIALS, MOCK_TOTP_CODE, mockState } from "../mocks/data";
import { renderApp, signInHalfSession } from "./utils";

async function fillLogin(user: ReturnType<typeof renderApp>["user"], password: string) {
  await user.type(await screen.findByLabelText("邮箱"), MOCK_CREDENTIALS.email);
  await user.type(screen.getByLabelText("密码"), password);
  await user.click(screen.getByRole("button", { name: "登录" }));
}

describe("管理员登录与两步验证", () => {
  it("首次登录未启用 2FA 时强制引导启用，完成后进入仪表盘", async () => {
    const { user } = renderApp();

    await fillLogin(user, MOCK_CREDENTIALS.password);

    // 强制注册页：二维码 + 密钥，且没有跳过入口。
    expect(await screen.findByRole("heading", { name: "启用两步验证" })).toBeInTheDocument();
    expect(await screen.findByTitle("两步验证二维码")).toBeInTheDocument();
    expect(screen.getByText("JBSWY3DPEHPK3PXP")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /跳过/ })).not.toBeInTheDocument();

    await user.type(screen.getByLabelText(/扫码后输入/), MOCK_TOTP_CODE);
    await user.click(screen.getByRole("button", { name: "启用并进入后台" }));

    expect(await screen.findByRole("heading", { name: "仪表盘" })).toBeInTheDocument();
    expect(mockState.session.totpEnabled).toBe(true);
  });

  it("已启用 2FA 时先给半会话，提交 TOTP 后才进入仪表盘", async () => {
    mockState.session.totpEnabled = true;
    const { user } = renderApp();

    await fillLogin(user, MOCK_CREDENTIALS.password);

    expect(await screen.findByRole("heading", { name: "两步验证" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "仪表盘" })).not.toBeInTheDocument();

    // 验证码错误时展示后端下发的固定文案。
    await user.type(screen.getByLabelText("验证码"), "000000");
    await user.click(screen.getByRole("button", { name: "验证" }));
    expect(await screen.findByText("两步验证码不正确。")).toBeInTheDocument();

    await user.clear(screen.getByLabelText("验证码"));
    await user.type(screen.getByLabelText("验证码"), MOCK_TOTP_CODE);
    await user.click(screen.getByRole("button", { name: "验证" }));

    expect(await screen.findByRole("heading", { name: "仪表盘" })).toBeInTheDocument();
  });

  it("半会话访问业务接口被拦截，退回两步验证页", async () => {
    signInHalfSession();
    renderApp("/users");

    expect(await screen.findByRole("heading", { name: "两步验证" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "用户" })).not.toBeInTheDocument();
    expect(screen.queryByRole("table")).not.toBeInTheDocument();
  });

  it("完整会话中途被降级为半会话时，业务页面立刻退回两步验证", async () => {
    mockState.session = { loggedIn: true, mfaPassed: true, totpEnabled: true };
    server.use(
      http.get("/api/admin/v1/metrics/overview", () =>
        HttpResponse.json(
          { error: { code: "mfa_required", message: "请输入两步验证码。" } },
          { status: 401 },
        ),
      ),
    );

    renderApp();

    expect(await screen.findByRole("heading", { name: "两步验证" })).toBeInTheDocument();
  });

  it("登录失败逐字展示后端的防枚举文案", async () => {
    const { user } = renderApp();
    await fillLogin(user, "wrong-password-1");

    expect(await screen.findByText("邮箱或密码不正确。")).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "仪表盘" })).not.toBeInTheDocument();
  });

  it("邮箱格式非法时做字段级校验，不发请求", async () => {
    const { user } = renderApp();

    await user.type(await screen.findByLabelText("邮箱"), "not-an-email");
    await user.type(screen.getByLabelText("密码"), "whatever12");
    await user.click(screen.getByRole("button", { name: "登录" }));

    expect(await screen.findByText("请输入有效的邮箱地址。")).toBeInTheDocument();
  });

  it("退出登录回到登录页", async () => {
    mockState.session = { loggedIn: true, mfaPassed: true, totpEnabled: true };
    const { user } = renderApp();

    await screen.findByRole("heading", { name: "仪表盘" });
    await user.click(screen.getByRole("button", { name: "退出登录" }));

    await waitFor(() => {
      expect(screen.getByRole("heading", { name: "CC避风港 运营后台" })).toBeInTheDocument();
    });
  });
});
