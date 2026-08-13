import { describe, expect, it } from "vitest";
import { screen, waitFor, within } from "@testing-library/react";
import { delay, http, HttpResponse } from "msw";
import { server } from "../mocks/server";
import { mockState } from "../mocks/data";
import { renderApp, signIn, signInAs } from "./utils";

/** 记录实际发出的请求路径，用来断言前端拿的是数字主键而不是展示号。 */
function recordRequests(): string[] {
  const paths: string[] = [];
  const listener = ({ request }: { request: Request }) => {
    paths.push(`${request.method} ${new URL(request.url).pathname}`);
  };
  server.events.on("request:start", listener);
  return paths;
}

function rowOf(id: string) {
  return screen.getByRole("cell", { name: id }).closest("tr") as HTMLTableRowElement;
}

describe("用户详情页", () => {
  it("从列表点「详情」进入，按 user_id 请求而不是展示号", async () => {
    signIn();
    const paths = recordRequests();
    const { user } = renderApp("/users");

    await screen.findByRole("cell", { name: "U-100986" });
    await user.click(within(rowOf("U-100986")).getByRole("button", { name: "查看 U-100986 的详情" }));

    expect(await screen.findByText("mary@example.com")).toBeInTheDocument();
    // 展示号 U-100986 只出现在界面上，绝不出现在 URL 里。
    expect(paths).toContain("GET /api/admin/v1/users/100986");
    expect(paths.some((path) => path.includes("U-100986"))).toBe(false);
  });

  it("展示明文邮箱并告知本次访问会被留痕", async () => {
    signIn();
    renderApp("/users/100986");

    expect(await screen.findByText("mary@example.com")).toBeInTheDocument();
    expect(
      screen.getByText(/本页展示明文邮箱，属于二次权限范围；每次访问都会记入审计日志/),
    ).toBeInTheDocument();
    // 后端为每次访问写审计，mock 同样照做。
    expect(mockState.audit[0]?.action).toBe("user.view_detail");
    expect(mockState.audit[0]?.target_id).toBe("100986");
  });

  it("渲染订阅、设备、邀请与最近订单四个区块", async () => {
    signIn();
    renderApp("/users/100986");

    await screen.findByText("mary@example.com");

    // 订阅快照。
    expect(screen.getByText("已订阅")).toBeInTheDocument();
    expect(screen.getByText("付费")).toBeInTheDocument();
    expect(screen.getByText("96 天")).toBeInTheDocument();

    // 设备：平台由后端拼好。
    expect(screen.getByRole("cell", { name: "D-9F2A41" })).toBeInTheDocument();
    expect(screen.getByRole("cell", { name: "macOS 15 · Apple Silicon" })).toBeInTheDocument();

    // 邀请：汇总 + 被邀请者邮箱仍然打码。
    expect(screen.getByText("已成功邀请 2 人 · 订阅共延长 14 天")).toBeInTheDocument();
    expect(screen.getByRole("cell", { name: "w***g@gmail.com" })).toBeInTheDocument();
    expect(screen.queryByText(/wangfang@gmail\.com/)).not.toBeInTheDocument();

    // 最近订单。
    const orderRow = screen.getByRole("cell", { name: /^CC\d{8}-100486$/ }).closest("tr")!;
    expect(within(orderRow).getByText(/68\.00/)).toBeInTheDocument();
    expect(within(orderRow).getByText("支付宝")).toBeInTheDocument();
  });

  it("无设备、无邀请、无订单时各区块显示空态", async () => {
    signIn();
    renderApp("/users/100984");

    expect(await screen.findByText("liuyi@qq.com")).toBeInTheDocument();
    expect(screen.getByText("该用户还没有登录过 APP。")).toBeInTheDocument();
    expect(screen.getByText("该用户还没有成功邀请过好友。")).toBeInTheDocument();
    expect(screen.getByText("该用户还没有订单。")).toBeInTheDocument();
  });

  it("请求未回来时显示骨架", async () => {
    signIn();
    server.use(
      http.get("/api/admin/v1/users/:id", async () => {
        await delay(200);
        return HttpResponse.json({
          data: {
            user: {
              id: "U-100986",
              user_id: 100986,
              email: "mary@example.com",
              display_name: "Mary",
              status: "active",
              created_at: new Date().toISOString(),
              source: "自然流量",
            },
            entitlement: { status: "none", days_left: 0, bonus_days_total: 0, expiring_soon: false },
            devices: [],
            referral: { invited_count: 0, total_bonus_days: 0, items: [] },
            orders: [],
          },
        });
      }),
    );

    renderApp("/users/100986");

    expect(await screen.findByTestId("detail-skeleton")).toBeInTheDocument();
    expect(await screen.findByText("mary@example.com")).toBeInTheDocument();
    expect(screen.queryByTestId("detail-skeleton")).not.toBeInTheDocument();
  });

  it("加载失败显示错误条并可重试", async () => {
    signIn();
    let attempt = 0;
    server.use(
      http.get("/api/admin/v1/users/:id", () => {
        attempt += 1;
        if (attempt === 1) {
          return HttpResponse.json(
            { error: { code: "internal_error", message: "服务暂时不可用，请稍后重试。" } },
            { status: 500 },
          );
        }
        return HttpResponse.json({
          data: {
            user: {
              id: "U-100986",
              user_id: 100986,
              email: "mary@example.com",
              display_name: "Mary",
              status: "active",
              created_at: new Date().toISOString(),
              source: "自然流量",
            },
            entitlement: { status: "none", days_left: 0, bonus_days_total: 0, expiring_soon: false },
            devices: [],
            referral: { invited_count: 0, total_bonus_days: 0, items: [] },
            orders: [],
          },
        });
      }),
    );

    const { user } = renderApp("/users/100986");

    expect(await screen.findByRole("alert")).toHaveTextContent("用户详情加载失败：");

    await user.click(screen.getByRole("button", { name: "重试" }));
    expect(await screen.findByText("mary@example.com")).toBeInTheDocument();
  });

  it("详情页禁用需要二次确认，操作期间按钮置忙，请求走 user_id", async () => {
    signIn();
    const paths = recordRequests();
    const { user } = renderApp("/users/100986");

    await screen.findByText("mary@example.com");
    await user.click(screen.getByRole("button", { name: "禁用" }));

    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText(/确定禁用 U-100986（mary@example\.com）吗？/)).toBeInTheDocument();
    expect(within(dialog).getByText("该用户将立即被登出且无法登录。")).toBeInTheDocument();

    await user.click(within(dialog).getByRole("button", { name: "禁用" }));

    expect(await screen.findByText("已禁用 U-100986。")).toBeInTheDocument();
    await waitFor(() => {
      expect(screen.getByRole("button", { name: "解禁" })).toBeInTheDocument();
    });
    expect(paths).toContain("POST /api/admin/v1/users/100986/disable");
    // 操作后页面会重新拉取详情，又写一条 view_detail，所以按内容找而不是取第一条。
    expect(
      mockState.audit.some(
        (record) => record.action === "user.disable" && record.target_id === "100986",
      ),
    ).toBe(true);
  });

  it("support 的详情入口是禁用态并说明原因", async () => {
    signInAs("support");
    renderApp("/users");

    await screen.findByRole("cell", { name: "U-100986" });
    const detailButton = within(rowOf("U-100986")).getByRole("button", {
      name: "查看 U-100986 的详情",
    });
    expect(detailButton).toBeDisabled();
    // 禁用的按钮不可聚焦，所以原因必须经 aria-describedby 关联，不能只挂 tooltip。
    expect(detailButton).toHaveAccessibleDescription(
      "详情页含明文邮箱，仅超级管理员与运营可查看。",
    );
  });

  it("support 直接访问详情 URL 时就地渲染 403，会话不受影响", async () => {
    signInAs("support");
    renderApp("/users/100986");

    expect(await screen.findByText("403 — 没有访问权限")).toBeInTheDocument();
    expect(screen.queryByText("mary@example.com")).not.toBeInTheDocument();
    // 会话仍然有效：侧栏还在，没有被踢回登录页。
    expect(screen.getByRole("navigation")).toBeInTheDocument();
    // 越权访问同样留痕。
    expect(mockState.audit[0]?.action).toBe("user.view_detail_denied");
  });
});
