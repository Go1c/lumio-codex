import { describe, expect, it } from "vitest";
import { screen, waitFor, within } from "@testing-library/react";
import { http, HttpResponse } from "msw";
import { server } from "../mocks/server";
import { mockState } from "../mocks/data";
import { orderChannelLabel } from "../lib/orderLabels";
import { renderApp, signIn, signInAs } from "./utils";

function rowOf(orderNo: string) {
  return screen.getByText(orderNo).closest("tr") as HTMLTableRowElement;
}

describe("订单与付款页", () => {
  it("页头常驻当日汇总", async () => {
    signIn();
    renderApp("/orders");

    expect(await screen.findByText("今日 2 笔 · ¥136")).toBeInTheDocument();
  });

  it("表格展示订单号、打码邮箱、右对齐金额与渠道", async () => {
    signIn();
    renderApp("/orders");

    const paid = mockState.orders[0]!;
    const row = within(await screen.findByText(paid.order_no).then((el) => el.closest("tr")!));
    expect(row.getByText("m***y@example.com")).toBeInTheDocument();

    const amountCell = row.getByText("¥68.00");
    expect(amountCell).toHaveClass("num");
    expect(row.getByText("支付宝")).toBeInTheDocument();
    expect(row.getByText("已支付")).toBeInTheDocument();
    expect(paid.order_no).toMatch(/^CC\d{8}-\d{6}$/);
    expect(orderChannelLabel("balance")).toBe("账户余额");
  });

  it("余额订单显示渠道但不提供退款", async () => {
    signIn();
    renderApp("/orders");

    const row = within(await screen.findByText(/CC\d{8}-100499/).then((el) => el.closest("tr")!));
    expect(row.getByText("账户余额")).toBeInTheDocument();
    expect(row.getByText("¥19.90")).toBeInTheDocument();
    expect(row.queryByRole("button", { name: "退款" })).toBeNull();
  });

  it("状态筛选 chips 生效，空结果给出提示", async () => {
    signIn();
    const { user } = renderApp("/orders");

    await screen.findByText("今日 2 笔 · ¥136");

    await user.click(screen.getByRole("button", { name: "已退款" }));
    await waitFor(() => {
      expect(screen.getAllByText("已退款").length).toBeGreaterThan(1);
    });

    server.use(
      http.get("/api/admin/v1/orders", () =>
        HttpResponse.json({
          data: { items: [], total: 0, page: 1, page_size: 20, today: { count: 0, amount_cents: 0 } },
        }),
      ),
    );
    await user.click(screen.getByRole("button", { name: "支付失败" }));

    expect(await screen.findByText("该状态下没有订单。")).toBeInTheDocument();
  });

  it("退款二次确认 → 退款中 → 已退款，全程 toast", async () => {
    signIn();
    const { user } = renderApp("/orders");

    const paid = mockState.orders[0]!;
    await screen.findByText(paid.order_no);
    await user.click(within(rowOf(paid.order_no)).getByRole("button", { name: "退款" }));

    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText(/确定对订单 .*发起退款吗？/)).toBeInTheDocument();
    expect(within(dialog).getByText(/退款成功后将扣回该订单对应的订阅天数/)).toBeInTheDocument();

    await user.click(within(dialog).getByRole("button", { name: "退款" }));

    expect(
      await screen.findByText(`退款已发起，订单 ${paid.order_no} 进入退款中。`),
    ).toBeInTheDocument();
    expect(await screen.findByText(`订单 ${paid.order_no} 退款完成。`)).toBeInTheDocument();

    await waitFor(() => {
      expect(within(rowOf(paid.order_no)).getByText("已退款")).toBeInTheDocument();
    });
    expect(mockState.audit[0]?.action).toBe("order.refund");
  });

  it("退款失败时回滚状态并 toast 后端文案", async () => {
    signIn();
    server.use(
      http.post("/api/admin/v1/orders/:orderNo/refund", () =>
        HttpResponse.json(
          { error: { code: "order_not_refundable", message: "该订单当前状态不支持退款。" } },
          { status: 409 },
        ),
      ),
    );

    const { user } = renderApp("/orders");
    const paid = mockState.orders[0]!;
    await screen.findByText(paid.order_no);

    await user.click(within(rowOf(paid.order_no)).getByRole("button", { name: "退款" }));
    await user.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "退款" }));

    expect(await screen.findByText("退款失败：该订单当前状态不支持退款。")).toBeInTheDocument();
    await waitFor(() => {
      expect(within(rowOf(paid.order_no)).getByText("已支付")).toBeInTheDocument();
    });
  });

  it("只有已支付订单可退款", async () => {
    signIn();
    renderApp("/orders");

    const refunded = mockState.orders.find((order) => order.status === "refunded")!;
    await screen.findByText(refunded.order_no);
    expect(within(rowOf(refunded.order_no)).queryByRole("button", { name: "退款" })).toBeNull();
  });

  it("support 的退款与导出都是禁用态并说明原因", async () => {
    signInAs("support");
    renderApp("/orders");

    const paid = mockState.orders[0]!;
    await screen.findByText(paid.order_no);

    const exportButton = screen.getByRole("button", { name: "导出 CSV" });
    expect(exportButton).toBeDisabled();
    expect(exportButton).toHaveAccessibleDescription(
      "导出会把大批用户邮箱带出系统，仅超级管理员与运营可执行。",
    );

    const refundButton = within(rowOf(paid.order_no)).getByRole("button", { name: "退款" });
    expect(refundButton).toBeDisabled();
    expect(refundButton).toHaveAccessibleDescription(
      "退款不可撤销并会扣回订阅天数，仅超级管理员与运营可执行。",
    );

    // 只读能力不受影响：列表与当日汇总照常。
    expect(screen.getByText("今日 2 笔 · ¥136")).toBeInTheDocument();
  });

  it.each(["owner", "ops"] as const)("%s 可以退款与导出", async (role) => {
    signInAs(role);
    renderApp("/orders");

    const paid = mockState.orders[0]!;
    await screen.findByText(paid.order_no);

    expect(screen.getByRole("button", { name: "导出 CSV" })).toBeEnabled();
    expect(within(rowOf(paid.order_no)).getByRole("button", { name: "退款" })).toBeEnabled();
  });

  it("导出被后端 403 挡下时就地说明，不踢回登录页", async () => {
    signInAs("owner");
    server.use(
      http.get("/api/admin/v1/orders/export", () =>
        HttpResponse.json({ error: { code: "forbidden", message: "没有访问权限。" } }, { status: 403 }),
      ),
    );

    const { user } = renderApp("/orders");
    await screen.findByText("今日 2 笔 · ¥136");
    await user.click(screen.getByRole("button", { name: "导出 CSV" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "导出会把大批用户邮箱带出系统，仅超级管理员与运营可执行。",
    );
    expect(screen.getByRole("navigation")).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "403 — 没有访问权限" })).not.toBeInTheDocument();
  });

  it("导出 CSV 按当前筛选调用后端", async () => {
    signIn();
    let exportedStatus = "";
    server.use(
      http.get("/api/admin/v1/orders/export", ({ request }) => {
        exportedStatus = new URL(request.url).searchParams.get("status") ?? "";
        return new HttpResponse("\uFEFF订单号\n", { headers: { "Content-Type": "text/csv" } });
      }),
    );

    const { user } = renderApp("/orders");
    await screen.findByText("今日 2 笔 · ¥136");

    await user.click(screen.getByRole("button", { name: "已支付" }));
    await user.click(screen.getByRole("button", { name: "导出 CSV" }));

    await waitFor(() => expect(exportedStatus).toBe("paid"));
    expect(await screen.findByText("已导出当前筛选的订单 CSV。")).toBeInTheDocument();
  });
});
