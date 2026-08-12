import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { DEMO_CODE, DEMO_PASSWORD, SCENARIO_EMAILS, db } from "@/mocks/db";
import { renderApp } from "@/test/utils";

/** 4.5 → 4.6：注册 → 验证码 → 成功页。 */

async function fillSignupForm(user: ReturnType<typeof renderApp>["user"], email: string) {
  await user.type(screen.getByLabelText("邮箱"), email);
  await user.type(screen.getByLabelText("密码"), DEMO_PASSWORD);
}

function codeBoxes() {
  return Array.from({ length: 6 }, (_, index) =>
    screen.getByLabelText(`第 ${index + 1} 位，共 6 位`),
  ) as HTMLInputElement[];
}

describe("注册页 /signup", () => {
  it("表单不完整时主按钮 disabled（disabled 态）", async () => {
    renderApp("/signup");
    expect(await screen.findByRole("button", { name: "创建账号" })).toBeDisabled();
  });

  it("邮箱格式错误 blur 后 inline 提示", async () => {
    const { user } = renderApp("/signup");
    const email = await screen.findByLabelText("邮箱");

    await user.type(email, "not-an-email");
    await user.tab();

    expect(await screen.findByText("请输入有效的邮箱地址。")).toBeInTheDocument();
  });

  it("邮箱已注册时 inline 提示并给出登录 / 找回密码入口", async () => {
    const { user } = renderApp("/signup");
    await fillSignupForm(user, SCENARIO_EMAILS.taken);
    await user.click(screen.getByRole("button", { name: "创建账号" }));

    const inlineError = await screen.findByText(/该邮箱已注册。/);
    const scope = within(inlineError);
    expect(scope.getByRole("link", { name: "登录" })).toHaveAttribute("href", "/login");
    expect(scope.getByRole("link", { name: "找回密码" })).toHaveAttribute("href", "/forgot-password");
  });

  it("频率限制用 toast 展示服务端 6.2 节文案", async () => {
    const { user } = renderApp("/signup");
    await fillSignupForm(user, SCENARIO_EMAILS.rateLimited);
    await user.click(screen.getByRole("button", { name: "创建账号" }));

    expect(await screen.findByText("尝试次数过多，请 1 分钟后再试。")).toBeInTheDocument();
  });

  it("注册成功后进入验证页并带上邮箱", async () => {
    const { user } = renderApp("/signup");
    await fillSignupForm(user, SCENARIO_EMAILS.ok);
    await user.click(screen.getByRole("button", { name: "创建账号" }));

    expect(await screen.findByRole("heading", { name: "请查收邮件" })).toBeInTheDocument();
    expect(screen.getByText(SCENARIO_EMAILS.ok)).toBeInTheDocument();
  });
});

describe("邮箱验证页 /verify-email", () => {
  const route = `/verify-email?email=${encodeURIComponent(SCENARIO_EMAILS.ok)}`;

  it("缺少邮箱参数时给出 empty 态与返回注册入口", async () => {
    renderApp("/verify-email");
    expect(await screen.findByText("缺少邮箱信息，请重新注册或从邮件中的链接进入。")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "返回注册" })).toBeInTheDocument();
  });

  it("粘贴验证码自动分配到 6 格并自动提交，成功后展示成功态", async () => {
    const { user } = renderApp(route);
    const boxes = codeBoxes();

    await user.click(boxes[0]);
    await user.paste(DEMO_CODE);

    expect(await screen.findByRole("heading", { name: "邮箱验证成功" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "下载 macOS 版" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "打开CC避风港 APP" })).toBeInTheDocument();
  });

  it("逐格输入填满即自动提交", async () => {
    const { user } = renderApp(route);
    const boxes = codeBoxes();

    await user.click(boxes[0]);
    await user.keyboard(DEMO_CODE);

    expect(await screen.findByRole("heading", { name: "邮箱验证成功" })).toBeInTheDocument();
  });

  it("验证码错误时剩余次数逐次递减，耗尽后转为过期态且格子禁用", async () => {
    const { user } = renderApp(route);

    for (const remaining of [4, 3, 2, 1]) {
      await user.click(codeBoxes()[0]);
      await user.paste("000000");
      expect(await screen.findByText(`验证码不正确，还剩 ${remaining} 次尝试机会。`)).toBeInTheDocument();
    }

    await user.click(codeBoxes()[0]);
    await user.paste("000000");

    expect(await screen.findByText("该验证码已过期，请重新发送。")).toBeInTheDocument();
    await waitFor(() => expect(codeBoxes()[0]).toBeDisabled());
  });

  it("重发按钮有 60 秒冷却，倒计时结束后可点击并 toast", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    renderApp(route);

    expect(await screen.findByText("60 秒后可重新发送")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "重新发送验证码" })).not.toBeInTheDocument();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(60_000);
    });

    await user.click(screen.getByRole("button", { name: "重新发送验证码" }));

    expect(await screen.findByText("验证码已发送，请查收邮件。")).toBeInTheDocument();
    expect(screen.getByText("60 秒后可重新发送")).toBeInTheDocument();
  });

  it("验证码已耗尽时首次提交即进入 disabled 态", async () => {
    db.codeAttemptsRemaining = 0;
    const { user } = renderApp(route);

    await user.click(codeBoxes()[0]);
    await user.paste("000000");

    expect(await screen.findByText("该验证码已过期，请重新发送。")).toBeInTheDocument();
    await waitFor(() => expect(codeBoxes()[0]).toBeDisabled());
  });
});

afterEach(() => {
  vi.useRealTimers();
});
