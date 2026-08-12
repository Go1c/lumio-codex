import { act, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { LoginPage } from "./LoginPage";
import { renderWithProviders } from "../test/render";

function setup(options: Parameters<typeof renderWithProviders>[1] = {}, props = {}) {
  const onSignedIn = vi.fn();
  const onUseOffline = vi.fn();
  const harness = renderWithProviders(
    <LoginPage
      onSignedIn={onSignedIn}
      onUseOffline={onUseOffline}
      canUseOffline={false}
      {...props}
    />,
    options,
  );
  return { ...harness, onSignedIn, onUseOffline };
}

describe("APP 登录页（5.1）", () => {
  it("待发起态只有一个按钮，没有任何邮箱或密码输入框", () => {
    setup();

    expect(screen.getByRole("button", { name: "通过浏览器登录 ↗" })).toBeInTheDocument();
    expect(
      screen.getByText(/应用本身不收集你的密码/),
    ).toBeInTheDocument();
    expect(document.querySelectorAll("input")).toHaveLength(0);
  });

  it("点击后进入等待授权态，提供重新打开浏览器与取消", async () => {
    const { user, api } = setup();

    await user.click(screen.getByRole("button", { name: "通过浏览器登录 ↗" }));

    expect(await screen.findByText("等待浏览器授权…")).toBeInTheDocument();
    expect(
      screen.getByText("已在浏览器中打开登录页。完成登录并点击「授权」后，这里会自动进入。"),
    ).toBeInTheDocument();
    expect(api.opened).toHaveLength(1);

    await user.click(screen.getByRole("button", { name: "重新打开浏览器" }));
    expect(api.opened).toHaveLength(2);

    await user.click(screen.getByRole("button", { name: "取消" }));
    expect(await screen.findByRole("button", { name: "通过浏览器登录 ↗" })).toBeInTheDocument();
  });

  it("授权成功后回调上层，并按邀请归因弹出试用祝贺", async () => {
    const { user, api, onSignedIn } = setup({ invited: true });

    await user.click(screen.getByRole("button", { name: "通过浏览器登录 ↗" }));
    act(() => api.finishBrowserLogin());

    await waitFor(() => expect(onSignedIn).toHaveBeenCalledTimes(1));
    expect(await screen.findByText(/🎁 首月免费试用已开通，有效期至 \d{4}年\d{1,2}月\d{1,2}日。/))
      .toBeInTheDocument();
  });

  it("超时态显示错误条、重试与手动粘贴授权码兜底", async () => {
    const { user, api, onSignedIn } = setup({ authTimesOut: true });

    await user.click(screen.getByRole("button", { name: "通过浏览器登录 ↗" }));
    act(() => api.finishBrowserLogin());

    expect(
      await screen.findByText(
        "等待授权超时。浏览器可能没有打开，或你尚未在浏览器中完成登录。",
      ),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "重试" })).toBeInTheDocument();

    const submit = screen.getByRole("button", { name: "使用授权码登录" });
    expect(submit).toBeDisabled();

    await user.type(screen.getByLabelText(/授权码/), "pasted-code");
    await user.click(submit);

    await waitFor(() => expect(onSignedIn).toHaveBeenCalledTimes(1));
  });

  it("手动授权码无效时把服务端文案原样显示出来", async () => {
    const { user, api } = setup({ authTimesOut: true });

    await user.click(screen.getByRole("button", { name: "通过浏览器登录 ↗" }));
    act(() => api.finishBrowserLogin());
    await screen.findByRole("button", { name: "重试" });

    await user.type(screen.getByLabelText(/授权码/), "invalid");
    await user.click(screen.getByRole("button", { name: "使用授权码登录" }));

    expect(await screen.findByText("授权码无效或已过期，请重新登录。")).toBeInTheDocument();
  });

  it("网络不可达且本地有缓存项目时，才提供「离线使用」入口", async () => {
    const { user, api, onUseOffline } = setup({}, { canUseOffline: true });

    await user.click(screen.getByRole("button", { name: "通过浏览器登录 ↗" }));
    act(() => api.failBrowserLogin());

    expect(await screen.findByText("无法连接服务器。")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "离线使用" }));
    expect(onUseOffline).toHaveBeenCalled();
  });

  it("会话过期的提示逐字使用 6.2 固定文案", () => {
    setup({}, { initialMessage: "登录已过期，请重新登录。" });
    expect(screen.getByText("登录已过期，请重新登录。")).toBeInTheDocument();
  });
});
