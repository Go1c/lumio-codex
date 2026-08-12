import { screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { VALID_RESET_TOKEN } from "@/mocks/db";
import { renderApp } from "@/test/utils";

/** 4.8 忘记密码与重设密码。 */

describe("忘记密码 /forgot-password", () => {
  it("提交后展示 6.2 节恒定回执，且回执与邮箱是否注册无关", async () => {
    const { user } = renderApp("/forgot-password");

    await user.type(await screen.findByLabelText("邮箱"), "nobody@example.com");
    await user.click(screen.getByRole("button", { name: "发送重设链接" }));

    expect(
      await screen.findByText("如 nobody@example.com 已注册账号，你将很快收到重设链接。"),
    ).toBeInTheDocument();
  });

  it("回执态下重复提交按钮进入 60 秒冷却", async () => {
    const { user } = renderApp("/forgot-password");

    await user.type(await screen.findByLabelText("邮箱"), "mary@example.com");
    await user.click(screen.getByRole("button", { name: "发送重设链接" }));

    expect(await screen.findByRole("button", { name: /秒后可重新发送/ })).toBeDisabled();
  });

  it("邮箱格式不合法时按钮保持 disabled", async () => {
    const { user } = renderApp("/forgot-password");

    await user.type(await screen.findByLabelText("邮箱"), "not-an-email");
    expect(screen.getByRole("button", { name: "发送重设链接" })).toBeDisabled();
  });
});

describe("重设密码 /reset-password", () => {
  it("链接失效时展示失效态与「重新申请链接」", async () => {
    renderApp("/reset-password?token=expired-token");

    expect(await screen.findByRole("heading", { name: "链接已失效" })).toBeInTheDocument();
    expect(await screen.findByText("该链接已过期或已被使用。")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "重新申请链接" })).toHaveAttribute(
      "href",
      "/forgot-password",
    );
  });

  it("缺少 token 时同样落在失效态", async () => {
    renderApp("/reset-password");
    expect(await screen.findByRole("heading", { name: "链接已失效" })).toBeInTheDocument();
  });

  it("token 有效时可设置新密码，成功后提示所有设备已退出", async () => {
    const { user } = renderApp(`/reset-password?token=${VALID_RESET_TOKEN}`);

    const password = await screen.findByLabelText("新密码");
    await user.type(password, "Password123");
    await user.type(screen.getByLabelText("确认新密码"), "Password123");
    await user.click(screen.getByRole("button", { name: "更新密码" }));

    expect(await screen.findByText("密码已更新，所有设备已退出登录。")).toBeInTheDocument();
  });

  it("两次密码不一致时 inline 报错且不能提交", async () => {
    const { user } = renderApp(`/reset-password?token=${VALID_RESET_TOKEN}`);

    await user.type(await screen.findByLabelText("新密码"), "Password123");
    await user.type(screen.getByLabelText("确认新密码"), "Password124");

    expect(await screen.findByText("两次输入的密码不一致。")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "更新密码" })).toBeDisabled();
  });
});
