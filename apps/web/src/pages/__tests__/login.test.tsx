import { act, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { DEMO_PASSWORD, SCENARIO_EMAILS } from "@/mocks/db";
import { renderApp } from "@/test/utils";

/** 4.7 登录页：3.2 状态机的四种失败分支各走一遍。 */

async function login(
  user: ReturnType<typeof renderApp>["user"],
  email: string,
  password = DEMO_PASSWORD,
) {
  await user.type(await screen.findByLabelText("邮箱"), email);
  await user.type(screen.getByLabelText("密码"), password);
  await user.click(screen.getByRole("button", { name: "登录" }));
}

describe("登录页 /login", () => {
  afterEach(() => vi.useRealTimers());

  it("凭据错误显示统一防枚举文案，保留邮箱、清空密码", async () => {
    const { user } = renderApp("/login");
    await login(user, SCENARIO_EMAILS.ok, "WrongPassword1");

    expect(await screen.findByText("邮箱或密码不正确。")).toBeInTheDocument();
    expect(screen.getByLabelText("邮箱")).toHaveValue(SCENARIO_EMAILS.ok);
    expect(screen.getByLabelText("密码")).toHaveValue("");
  });

  it("邮箱未验证时给出错误条与「重新发送验证邮件」按钮", async () => {
    const { user } = renderApp("/login");
    await login(user, SCENARIO_EMAILS.unverified);

    expect(await screen.findByText("你的邮箱尚未验证。")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "重新发送验证邮件" }));
    expect(await screen.findByRole("heading", { name: "请查收邮件" })).toBeInTheDocument();
  });

  it("账号锁定时展示限频文案，按钮禁用到倒计时结束", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    renderApp("/login");

    await user.type(await screen.findByLabelText("邮箱"), SCENARIO_EMAILS.locked);
    await user.type(screen.getByLabelText("密码"), DEMO_PASSWORD);
    await user.click(screen.getByRole("button", { name: "登录" }));

    expect(await screen.findByText(/尝试次数过多，请 15 分钟后再试。/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "登录" })).toBeDisabled();

    // details.retry_after_seconds = 900：倒计时归零后按钮恢复可用。
    await act(async () => {
      await vi.advanceTimersByTimeAsync(900_000);
    });
    expect(screen.getByRole("button", { name: "登录" })).toBeEnabled();
  });

  it("账号停用时展示错误条并禁用提交", async () => {
    const { user } = renderApp("/login");
    await login(user, SCENARIO_EMAILS.disabled);

    expect(await screen.findByText("账号已停用，请联系支持。")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "登录" })).toBeDisabled();
  });

  it("登录成功后进入账户中心", async () => {
    const { user } = renderApp("/login");
    await login(user, SCENARIO_EMAILS.ok);

    expect(await screen.findByRole("heading", { name: "账户中心" })).toBeInTheDocument();
  });

  it("带 next 参数时登录后回到原页面（授权页往返）", async () => {
    const next = "/authorize?client_id=cchaven-desktop";
    const { user } = renderApp(`/login?next=${encodeURIComponent(next)}`);
    await login(user, SCENARIO_EMAILS.ok);

    expect(await screen.findByRole("heading", { name: /无法完成授权|授权 CC避风港 APP 访问你的账号/ })).toBeInTheDocument();
  });
});
