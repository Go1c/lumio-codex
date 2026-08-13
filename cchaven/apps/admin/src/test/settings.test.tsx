import { describe, expect, it } from "vitest";
import { screen, waitFor, within } from "@testing-library/react";
import { http, HttpResponse } from "msw";
import { server } from "../mocks/server";
import { mockState } from "../mocks/data";
import { renderApp, signIn, signInAs } from "./utils";

describe("运营配置页", () => {
  it("回填当前配置", async () => {
    signIn();
    renderApp("/settings");

    await waitFor(() => {
      expect(screen.getByLabelText(/邀请者奖励/)).toHaveValue("7");
    });
    expect(screen.getByLabelText(/被邀请者免费试用时长/)).toHaveValue("30");
    expect(screen.getByLabelText(/包月价格/)).toHaveValue("68");
  });

  it("保存后提示实时生效，并按点号 key 提交", async () => {
    signIn();
    let sentBody: Record<string, unknown> = {};
    server.use(
      http.put("/api/admin/v1/configs", async ({ request }) => {
        sentBody = (await request.json()) as Record<string, unknown>;
        return HttpResponse.json({
          data: {
            invite_reward_days: 14,
            invite_trial_days: 30,
            pricing_monthly: { amount_cents: 6800, currency: "CNY" },
          },
        });
      }),
    );

    const { user } = renderApp("/settings");
    const rewardInput = await screen.findByLabelText(/邀请者奖励/);

    await user.clear(rewardInput);
    await user.type(rewardInput, "14");
    await user.click(screen.getByRole("button", { name: "保存配置" }));

    expect(await screen.findByText("配置已保存，官网与 APP 端实时生效。")).toBeInTheDocument();
    expect(sentBody).toEqual({ "invite.reward_days": 14 });
  });

  it("奖励天数配 0 时说明前端文案会自动隐藏", async () => {
    signIn();
    const { user } = renderApp("/settings");

    const rewardInput = await screen.findByLabelText(/邀请者奖励/);
    expect(screen.queryByRole("note")).not.toBeInTheDocument();

    await user.clear(rewardInput);
    await user.type(rewardInput, "0");

    expect(
      screen.getByText(
        "当前配置为 0：邀请者奖励已关闭，前台不再展示「每邀请 1 人延长 X 天」相关文案。",
      ),
    ).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "保存配置" }));
    await screen.findByText("配置已保存，官网与 APP 端实时生效。");
    expect(mockState.config.invite_reward_days).toBe(0);
  });

  it("价格非法时给字段级错误且不提交", async () => {
    signIn();
    const { user } = renderApp("/settings");

    const priceInput = await screen.findByLabelText(/包月价格/);
    await user.clear(priceInput);
    await user.type(priceInput, "0");
    await user.click(screen.getByRole("button", { name: "保存配置" }));

    expect(await screen.findByText("请输入大于 0 的金额，最多两位小数。")).toBeInTheDocument();
    expect(mockState.config.pricing_monthly.amount_cents).toBe(6800);
  });

  it("support 看到只读版本：值可见、保存禁用并说明原因", async () => {
    signInAs("support");
    renderApp("/settings");

    // 整页 403 是错的：客服要能回答「现在奖励几天、包月多少钱」。
    await waitFor(() => {
      expect(screen.getByLabelText(/邀请者奖励/)).toHaveValue("7");
    });
    expect(screen.getByLabelText(/包月价格/)).toHaveValue("68");
    expect(screen.getByLabelText(/包月价格/)).toHaveAttribute("readonly");
    expect(screen.queryByRole("heading", { name: "403 — 没有访问权限" })).not.toBeInTheDocument();

    const save = screen.getByRole("button", { name: "保存配置" });
    expect(save).toBeDisabled();
    expect(save).toHaveAccessibleDescription(
      "当前账号为只读权限：可以查看现行配置，但不能修改。改价格与奖励天数仅超级管理员与运营可执行。",
    );
  });

  it.each(["owner", "ops"] as const)("%s 可以保存配置", async (role) => {
    signInAs(role);
    renderApp("/settings");

    await waitFor(() => {
      expect(screen.getByLabelText(/邀请者奖励/)).toHaveValue("7");
    });
    expect(screen.getByRole("button", { name: "保存配置" })).toBeEnabled();
    expect(screen.getByLabelText(/包月价格/)).not.toHaveAttribute("readonly");
  });

  it("保存被后端 403 挡下时就地说明，不踢回登录页", async () => {
    signInAs("owner");
    server.use(
      http.put("/api/admin/v1/configs", () =>
        HttpResponse.json({ error: { code: "forbidden", message: "没有访问权限。" } }, { status: 403 }),
      ),
    );

    const { user } = renderApp("/settings");
    const rewardInput = await screen.findByLabelText(/邀请者奖励/);
    await user.clear(rewardInput);
    await user.type(rewardInput, "14");
    await user.click(screen.getByRole("button", { name: "保存配置" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("当前账号为只读权限");
    // 仍在配置页，没有整屏 403，也没有回登录页。
    expect(screen.getByLabelText(/邀请者奖励/)).toBeInTheDocument();
    expect(screen.getByRole("navigation")).toBeInTheDocument();
  });

  it("审计日志作为子区块展示操作人、时间与前后值", async () => {
    signIn();
    renderApp("/settings");

    const section = await screen.findByRole("heading", { name: "操作审计日志" });
    const card = section.closest("section") as HTMLElement;

    // 动作下拉里也有同名选项，所以按单元格取，避免歧义。
    expect(await within(card).findByRole("cell", { name: "禁用用户" })).toBeInTheDocument();
    expect(within(card).getByRole("cell", { name: "订单退款" })).toBeInTheDocument();
    expect(within(card).getAllByText("管理员 #1").length).toBeGreaterThan(0);
    expect(within(card).getByText(/"status":"active"/)).toBeInTheDocument();
  });

  it("审计日志可按动作筛选", async () => {
    signIn();
    const { user } = renderApp("/settings");

    const card = (await screen.findByRole("heading", { name: "操作审计日志" })).closest(
      "section",
    ) as HTMLElement;
    await within(card).findByRole("cell", { name: "禁用用户" });

    await user.selectOptions(within(card).getByLabelText("动作"), "order.refund");

    await waitFor(() => {
      expect(within(card).queryByRole("cell", { name: "禁用用户" })).not.toBeInTheDocument();
    });
    expect(within(card).getByRole("cell", { name: "订单退款" })).toBeInTheDocument();
  });

  it("审计日志可按操作人筛选，无匹配时给筛选专属空态", async () => {
    signIn();
    const { user } = renderApp("/settings");

    const card = (await screen.findByRole("heading", { name: "操作审计日志" })).closest(
      "section",
    ) as HTMLElement;
    await within(card).findByRole("cell", { name: "禁用用户" });

    await user.type(within(card).getByLabelText("操作人 ID"), "42");

    expect(await within(card).findByText("没有匹配的审计记录。")).toBeInTheDocument();

    await user.click(within(card).getByRole("button", { name: "清除筛选" }));
    expect(await within(card).findByRole("cell", { name: "禁用用户" })).toBeInTheDocument();
  });
});
