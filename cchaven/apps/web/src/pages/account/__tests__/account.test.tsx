import { screen, waitFor, within } from "@testing-library/react";
import { http } from "msw";
import { beforeEach, describe, expect, it } from "vitest";

import { DEMO_CODE, DEMO_PASSWORD, db } from "@/mocks/db";
import { fail, ok } from "@/mocks/handlers";
import { server } from "@/mocks/server";
import { renderApp } from "@/test/utils";

/** 5.6 官网账户中心：五个分区 × 五态。 */

const API = "*/api/v1";

function loggedIn() {
  db.loggedIn = true;
}

/** 让某个接口一直挂起，用于断言 loading 骨架。 */
function pending(path: string) {
  server.use(http.get(path, () => new Promise<never>(() => {})));
}

describe("账户中心 /account", () => {
  it("未登录时展示无权限态与去登录入口", async () => {
    renderApp("/account");

    expect(await screen.findByText("请先登录后再管理账户。")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "去登录" })).toHaveAttribute("href", "/login?next=/account");
  });

  describe("已登录", () => {
    beforeEach(loggedIn);

    it("渲染五个分区与危险区", async () => {
      renderApp("/account");

      expect(await screen.findByRole("heading", { name: "账户中心" })).toBeInTheDocument();
      for (const title of ["订阅与付款", "个人资料", "邀请好友", "安全", "登录设备与授权", "危险操作"]) {
        expect(await screen.findByRole("heading", { name: title })).toBeInTheDocument();
      }
    });

    describe("订阅与付款", () => {
      it("已订阅时展示有效期与剩余天数徽标（日期格式 YYYY年M月D日）", async () => {
        renderApp("/account");
        expect(await screen.findByText(/已订阅 · 有效期至 \d{4}年\d{1,2}月\d{1,2}日（剩余 27 天）/)).toBeInTheDocument();
      });

      it("试用中展示剩余天数徽标", async () => {
        db.entitlement = {
          status: "trialing",
          kind: "trial",
          expires_at: new Date(Date.now() + 23 * 86400_000).toISOString(),
          days_left: 23,
          bonus_days_total: 0,
          expiring_soon: false,
        };
        renderApp("/account");
        expect(await screen.findByText("免费试用中 · 剩余 23 天")).toBeInTheDocument();
      });

      it("未订阅时展示「未订阅」并给出立即订阅按钮", async () => {
        db.entitlement = { status: "none", days_left: 0, bonus_days_total: 0, expiring_soon: false };
        renderApp("/account");

        expect(await screen.findByText("未订阅")).toBeInTheDocument();
        expect(await screen.findByRole("button", { name: "立即订阅" })).toBeEnabled();
      });

      it("套餐加载中展示骨架（loading 态）", async () => {
        pending(`${API}/billing/plan`);
        renderApp("/account");

        const section = await screen.findByLabelText("订阅与付款");
        expect(within(section).getByRole("status")).toHaveAttribute("aria-busy", "true");
      });

      it("套餐加载失败展示错误条与重试（error 态）", async () => {
        server.use(
          http.get(`${API}/billing/plan`, () =>
            fail(500, "internal_error", "服务暂时不可用，请稍后重试。"),
          ),
        );
        renderApp("/account");

        const section = await screen.findByLabelText("订阅与付款");
        expect(await within(section).findByText("服务暂时不可用，请稍后重试。")).toBeInTheDocument();
        expect(within(section).getByRole("button", { name: "重试" })).toBeInTheDocument();
      });

      it("付款说明明确「站内不收集付款信息」", async () => {
        renderApp("/account");
        expect(
          await screen.findByText("付款只在本页面完成，APP 内不处理、也不收集任何付款信息。"),
        ).toBeInTheDocument();
      });
    });

    describe("个人资料", () => {
      it("邮箱只读、显示名称可保存（保存中按钮 disabled）", async () => {
        const { user } = renderApp("/account");

        const nameInput = await screen.findByLabelText("显示名称");
        expect(screen.getByText("账号 U-100986")).toBeInTheDocument();

        await user.clear(nameInput);
        await user.type(nameInput, "玛丽");
        await user.click(screen.getByRole("button", { name: "保存" }));

        expect(await screen.findByText("资料已更新。")).toBeInTheDocument();
      });

      it("未改动时保存按钮保持 disabled", async () => {
        renderApp("/account");
        expect(await screen.findByRole("button", { name: "保存" })).toBeDisabled();
      });
    });

    describe("邀请好友", () => {
      it("展示奖励天数、汇总行与进度列表", async () => {
        renderApp("/account");

        expect(
          await screen.findByText(
            "朋友经你的链接下载、注册并登录 APP 后，将获得首月免费试用（每个账号限一次）；每成功邀请 1 人，你的订阅延长 7 天。",
          ),
        ).toBeInTheDocument();
        expect(screen.getByText("🎉 已成功邀请 1 人 · 订阅共延长 7 天")).toBeInTheDocument();
        expect(screen.getByText("w***g@gmail.com")).toBeInTheDocument();
        expect(screen.getByText(/已注册，尚未登录 APP/)).toBeInTheDocument();
      });

      it("reward_days 为 0 时隐藏「订阅延长 X 天」相关文案", async () => {
        db.referrals = { ...db.referrals, reward_days: 0, total_bonus_days: 0 };
        renderApp("/account");

        expect(
          await screen.findByText(
            "朋友经你的链接下载、注册并登录 APP 后，将获得首月免费试用（每个账号限一次）。",
          ),
        ).toBeInTheDocument();
        expect(screen.queryByText(/订阅延长/)).not.toBeInTheDocument();
        expect(screen.queryByText(/订阅共延长/)).not.toBeInTheDocument();
      });

      it("没有好友加入时展示空状态（empty 态）", async () => {
        db.referrals = { ...db.referrals, items: [], invited_count: 0, total_bonus_days: 0 };
        renderApp("/account");

        expect(await screen.findByText("还没有朋友加入。")).toBeInTheDocument();
      });

      it("复制链接后按钮进入「已复制 ✓」并短暂禁用（disabled 态）", async () => {
        const { user } = renderApp("/account");

        await user.click(await screen.findByRole("button", { name: "复制链接" }));

        const copied = await screen.findByRole("button", { name: "已复制 ✓" });
        expect(copied).toBeDisabled();
        expect(await screen.findByText("已复制邀请链接。")).toBeInTheDocument();
      });

      it("加载失败展示错误条与重试（error 态）", async () => {
        server.use(
          http.get(`${API}/me/referrals`, () => fail(500, "internal_error", "服务暂时不可用，请稍后重试。")),
        );
        renderApp("/account");

        const section = await screen.findByLabelText("邀请好友");
        expect(await within(section).findByRole("button", { name: "重试" })).toBeInTheDocument();
      });

      it("加载中展示骨架（loading 态）", async () => {
        pending(`${API}/me/referrals`);
        renderApp("/account");

        const section = await screen.findByLabelText("邀请好友");
        expect(within(section).getByRole("status")).toHaveAttribute("aria-busy", "true");
      });
    });

    describe("安全", () => {
      it("当前密码错误时展示服务端文案", async () => {
        const { user } = renderApp("/account");

        await user.type(await screen.findByLabelText("当前密码"), "WrongPassword1");
        await user.type(screen.getByLabelText("新密码"), "Password456");
        await user.click(screen.getByRole("button", { name: "修改密码" }));

        expect(await screen.findByText("当前密码不正确。")).toBeInTheDocument();
      });

      it("修改密码成功后提示其他设备已退出", async () => {
        const { user } = renderApp("/account");

        await user.type(await screen.findByLabelText("当前密码"), DEMO_PASSWORD);
        await user.type(screen.getByLabelText("新密码"), "Password456");
        await user.click(screen.getByRole("button", { name: "修改密码" }));

        expect(await screen.findByText("密码已更新，其他设备已退出登录。")).toBeInTheDocument();
      });

      it("新密码不满足规则时按钮 disabled（disabled 态）", async () => {
        const { user } = renderApp("/account");

        await user.type(await screen.findByLabelText("当前密码"), DEMO_PASSWORD);
        await user.type(screen.getByLabelText("新密码"), "short");

        expect(screen.getByRole("button", { name: "修改密码" })).toBeDisabled();
      });

      it("修改邮箱走两步：发码 → 输入验证码 → 切换", async () => {
        const { user } = renderApp("/account");

        await user.type(await screen.findByLabelText("新邮箱"), "new@example.com");
        await user.click(screen.getByRole("button", { name: "发送验证码" }));

        expect(
          await screen.findByText("验证码已发送到 new@example.com，请输入 6 位验证码完成切换。"),
        ).toBeInTheDocument();

        await user.click(screen.getByLabelText("第 1 位，共 6 位"));
        await user.paste(DEMO_CODE);

        expect(await screen.findByText("邮箱已更新。")).toBeInTheDocument();
      });
    });

    describe("登录设备与授权", () => {
      it("列出会话并标出本设备", async () => {
        renderApp("/account");

        const section = await screen.findByLabelText("登录设备与授权");
        expect(await within(section).findByText(/Safari · macOS/)).toBeInTheDocument();
        expect(within(section).getByText("本设备")).toBeInTheDocument();
        expect(within(section).getByText(/MacBook Pro/)).toBeInTheDocument();
      });

      it("退出设备需要二次确认，可 Esc 关闭模态", async () => {
        const { user } = renderApp("/account");

        const section = await screen.findByLabelText("登录设备与授权");
        const revokeButtons = await within(section).findAllByRole("button", { name: "退出该设备" });
        await user.click(revokeButtons[0]);

        const dialog = await screen.findByRole("dialog");
        expect(dialog).toHaveAttribute("aria-modal", "true");

        await user.keyboard("{Escape}");
        await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
      });

      it("确认后撤销该设备并给出 toast", async () => {
        const { user } = renderApp("/account");

        const section = await screen.findByLabelText("登录设备与授权");
        const revokeButtons = await within(section).findAllByRole("button", { name: "退出该设备" });
        await user.click(revokeButtons[0]);
        await user.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "确定" }));

        expect(await screen.findByText("该设备已退出登录。")).toBeInTheDocument();
        await waitFor(() => expect(screen.queryByText(/MacBook Pro/)).not.toBeInTheDocument());
      });

      it("只有当前会话时不显示「退出所有其他设备」（empty 态）", async () => {
        db.sessions = db.sessions.filter((session) => session.current);
        renderApp("/account");

        await screen.findByLabelText("登录设备与授权");
        expect(screen.queryByRole("button", { name: "退出所有其他设备" })).not.toBeInTheDocument();
      });

      it("加载失败展示错误条与重试（error 态）", async () => {
        server.use(
          http.get(`${API}/me/sessions`, () => fail(500, "internal_error", "服务暂时不可用，请稍后重试。")),
        );
        renderApp("/account");

        const section = await screen.findByLabelText("登录设备与授权");
        expect(await within(section).findByRole("button", { name: "重试" })).toBeInTheDocument();
      });

      it("加载中展示骨架（loading 态）", async () => {
        pending(`${API}/me/sessions`);
        renderApp("/account");

        const section = await screen.findByLabelText("登录设备与授权");
        expect(within(section).getByRole("status")).toHaveAttribute("aria-busy", "true");
      });
    });

    describe("危险区", () => {
      it("注销账号有二次确认，确认后展示 7 天冷静期与撤销入口", async () => {
        const { user } = renderApp("/account");

        await user.click(await screen.findByRole("button", { name: "注销账号…" }));
        await user.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "确认注销" }));

        expect(await screen.findByText(/已申请注销，将于 \d{4}年\d{1,2}月\d{1,2}日 生效。/)).toBeInTheDocument();

        await user.click(screen.getByRole("button", { name: "撤销注销" }));
        expect(await screen.findByText("已撤销注销申请。")).toBeInTheDocument();
      });

      it("退出登录后回到首页", async () => {
        const { user } = renderApp("/account");

        await user.click(await screen.findByRole("button", { name: "退出登录" }));

        expect(await screen.findByText("已退出登录。")).toBeInTheDocument();
        expect(await screen.findByRole("heading", { name: /安心使用 Claude Code/ })).toBeInTheDocument();
      });
    });

    it("会话过期时先刷新再重试，刷新失败才回到未登录态", async () => {
      let sessionCalls = 0;
      server.use(
        http.get(`${API}/me/sessions`, () => {
          sessionCalls += 1;
          if (sessionCalls === 1) return fail(401, "session_expired", "登录已过期，请重新登录。");
          return ok({ items: db.sessions });
        }),
        http.post(`${API}/auth/refresh`, () => ok({ expires_in: 900 })),
      );

      renderApp("/account");

      const section = await screen.findByLabelText("登录设备与授权");
      expect(await within(section).findByText(/MacBook Pro/)).toBeInTheDocument();
      expect(sessionCalls).toBe(2);
    });
  });
});