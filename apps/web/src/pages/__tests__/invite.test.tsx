import { screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { VALID_INVITE_CODE, db } from "@/mocks/db";
import { renderApp } from "@/test/utils";

/** 4.4 邀请落地页：有效 / 失效两态，失效也不阻断注册。 */

describe("邀请落地页 /i/{code}", () => {
  it("有效邀请码展示邀请人、首月免费与两个 CTA", async () => {
    renderApp(`/i/${VALID_INVITE_CODE}`);

    expect(await screen.findByRole("heading", { name: "Alex 邀请你使用CC避风港" })).toBeInTheDocument();
    expect(screen.getByText("注册并登录 APP，即享首月免费试用。")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "注册领取首月免费" })).toHaveAttribute("href", "/signup");
    expect(screen.getByRole("link", { name: "先下载 macOS 版" })).toHaveAttribute("href", "/download");
    expect(
      screen.getByText(`邀请码 ${VALID_INVITE_CODE} 已自动记录，无需手动输入。每个账号只可享用一次免费试用。`),
    ).toBeInTheDocument();
  });

  it("落地成功后归因生效，注册页据此显示邀请横幅", async () => {
    const { user } = renderApp(`/i/${VALID_INVITE_CODE}`);

    await screen.findByRole("heading", { name: "Alex 邀请你使用CC避风港" });
    // 真实后端在落地页下发 cch_ref cookie，mock 用状态位代替。
    expect(db.inviteAttributed).toBe(true);

    await user.click(screen.getByRole("link", { name: "注册领取首月免费" }));
    expect(
      await screen.findByText("🎁 Alex 邀请你使用CC避风港 — 注册并登录 APP 即享首月免费试用。"),
    ).toBeInTheDocument();
  });

  it("失效邀请码展示「此邀请链接已失效」但仍给出注册入口", async () => {
    renderApp("/i/deadbeef");

    expect(await screen.findByRole("heading", { name: "此邀请链接已失效" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "创建账号" })).toHaveAttribute("href", "/signup");
    expect(db.inviteAttributed).toBe(false);
  });
});
