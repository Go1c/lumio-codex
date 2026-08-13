import { describe, expect, it } from "vitest";
import { screen, within } from "@testing-library/react";
import { http, HttpResponse } from "msw";
import { server } from "../mocks/server";
import { mockState } from "../mocks/data";
import { renderApp, signIn } from "./utils";

describe("仪表盘", () => {
  it("渲染六张核心指标卡", async () => {
    signIn();
    renderApp();

    await screen.findByTestId("stat-dau");

    const labels = [
      "今日日活（DAU）",
      "今日新增注册",
      "付费订阅用户",
      "今日收入",
      "试用 → 付费转化率",
      "7 日留存",
    ];
    for (const label of labels) {
      expect(screen.getByText(label)).toBeInTheDocument();
    }

    expect(within(screen.getByTestId("stat-dau")).getByText("1,284")).toBeInTheDocument();
    expect(within(screen.getByTestId("stat-dau")).getByText("较昨日 +5.8%")).toBeInTheDocument();
    expect(within(screen.getByTestId("stat-signups")).getByText("其中经邀请 41 人")).toBeInTheDocument();
    expect(within(screen.getByTestId("stat-subscribers")).getByText("试用中 214 人")).toBeInTheDocument();
    // 收入按分转元展示，副标题是当日订单笔数。
    expect(within(screen.getByTestId("stat-revenue")).getByText("¥136")).toBeInTheDocument();
    expect(within(screen.getByTestId("stat-revenue")).getByText("2 笔订单")).toBeInTheDocument();
  });

  it("value 为 null 时显示「—」而不是 0", async () => {
    signIn();
    renderApp();

    const card = await screen.findByTestId("stat-trial_conversion");
    expect(within(card).getByText("—")).toBeInTheDocument();
    expect(within(card).queryByText("0%")).not.toBeInTheDocument();
    expect(within(card).queryByText("0.0%")).not.toBeInTheDocument();
    expect(within(card).queryByText("0")).not.toBeInTheDocument();
  });

  it("全部指标缺数时六张卡都显示「—」，副标题也不回落成 0", async () => {
    signIn();
    server.use(
      http.get("/api/admin/v1/metrics/overview", () =>
        HttpResponse.json({
          data: {
            dau: { value: null },
            signups: { value: null },
            subscribers: { value: null },
            revenue: { value: null },
            trial_conversion: { value: null },
            retention_d7: { value: null },
            generated_at: new Date().toISOString(),
          },
        }),
      ),
    );

    renderApp();
    await screen.findByRole("heading", { name: "仪表盘" });

    for (const key of ["dau", "signups", "subscribers", "revenue", "trial_conversion", "retention_d7"]) {
      const card = await screen.findByTestId(`stat-${key}`);
      expect(within(card).getAllByText(/—/).length).toBeGreaterThan(0);
    }
    expect(screen.getByText("其中经邀请 — 人")).toBeInTheDocument();
    expect(screen.getByText("较昨日 —")).toBeInTheDocument();
  });

  it("7 日留存下降时副标题转橙色", async () => {
    signIn();
    renderApp();

    const card = await screen.findByTestId("stat-retention_d7");
    const sub = within(card).getByText("较上周 -1.2%");
    expect(sub).toHaveClass("tone-down");
  });

  it("近 7 日日活柱状图末位标注「今天」并带数值", async () => {
    signIn();
    renderApp();

    await screen.findByRole("heading", { name: "近 7 日日活" });
    expect(screen.getByText("今天")).toBeInTheDocument();
    expect(screen.getByLabelText("今天：1,284")).toBeInTheDocument();
    expect(screen.getByText("980")).toBeInTheDocument();
  });

  it("三组分布图按占比渲染", async () => {
    signIn();
    renderApp();

    expect(
      await screen.findByRole("heading", { name: "使用平台分布（近 30 天活跃）" }),
    ).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "APP 版本分布" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "注册来源（近 30 天）" })).toBeInTheDocument();

    expect(screen.getByText("macOS · Apple Silicon")).toBeInTheDocument();
    expect(screen.getByText("78%")).toBeInTheDocument();
  });

  it("加载失败显示错误条，重试后恢复", async () => {
    signIn();
    let attempt = 0;
    server.use(
      http.get("/api/admin/v1/metrics/overview", () => {
        attempt += 1;
        if (attempt === 1) {
          return HttpResponse.json(
            { error: { code: "internal_error", message: "服务暂时不可用，请稍后重试。" } },
            { status: 500 },
          );
        }
        return HttpResponse.json({ data: mockState.overview });
      }),
    );

    const { user } = renderApp();

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "指标加载失败：服务暂时不可用，请稍后重试。",
    );

    await user.click(screen.getByRole("button", { name: "重试" }));
    expect(await screen.findByTestId("stat-dau")).toBeInTheDocument();
  });
});
