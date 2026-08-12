import { describe, expect, it } from "vitest";
import { screen, waitFor, within } from "@testing-library/react";
import { http, HttpResponse } from "msw";
import { server } from "../mocks/server";
import { mockState } from "../mocks/data";
import { renderApp, signIn, signInAs } from "./utils";

function rowOf(id: string) {
  return screen.getByRole("cell", { name: id }).closest("tr") as HTMLTableRowElement;
}

describe("用户页", () => {
  it("列出用户，邮箱打码、未登录 APP 显示占位", async () => {
    signIn();
    renderApp("/users");

    expect(await screen.findByRole("cell", { name: "U-100986" })).toBeInTheDocument();
    expect(screen.getByText("m***y@example.com")).toBeInTheDocument();
    expect(screen.getByText("—（未登录 APP）")).toBeInTheDocument();
    // 邀请来源带邀请者 ID。
    expect(screen.getAllByText("邀请（U-100986）").length).toBe(2);
    expect(screen.getByText("macOS 15 · Apple Silicon")).toBeInTheDocument();
  });

  it("订阅状态筛选 chips 生效", async () => {
    signIn();
    const { user } = renderApp("/users");

    await screen.findByRole("cell", { name: "U-100986" });

    await user.click(screen.getByRole("button", { name: "试用中" }));

    await waitFor(() => {
      expect(screen.queryByRole("cell", { name: "U-100986" })).not.toBeInTheDocument();
    });
    expect(screen.getByRole("cell", { name: "U-100985" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "试用中" })).toHaveAttribute("aria-pressed", "true");
  });

  it("搜索命中与空结果提示", async () => {
    signIn();
    const { user } = renderApp("/users");

    await screen.findByRole("cell", { name: "U-100986" });

    await user.type(screen.getByLabelText("搜索邮箱或用户 ID"), "100985");
    await waitFor(() => {
      expect(screen.queryByRole("cell", { name: "U-100986" })).not.toBeInTheDocument();
    });
    expect(screen.getByRole("cell", { name: "U-100985" })).toBeInTheDocument();

    await user.clear(screen.getByLabelText("搜索邮箱或用户 ID"));
    await user.type(screen.getByLabelText("搜索邮箱或用户 ID"), "查无此人");
    expect(await screen.findByText("没有匹配的用户。")).toBeInTheDocument();
  });

  it("禁用需要二次确认，并明确「立即被登出」的后果", async () => {
    signIn();
    const { user } = renderApp("/users");

    await screen.findByRole("cell", { name: "U-100986" });
    await user.click(within(rowOf("U-100986")).getByRole("button", { name: "禁用" }));

    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText(/确定禁用 U-100986（m\*\*\*y@example.com）吗？/)).toBeInTheDocument();
    expect(within(dialog).getByText("该用户将立即被登出且无法登录。")).toBeInTheDocument();

    await user.click(within(dialog).getByRole("button", { name: "禁用" }));

    expect(await screen.findByText("已禁用 U-100986。")).toBeInTheDocument();
    await waitFor(() => {
      expect(within(rowOf("U-100986")).getByText("已禁用")).toBeInTheDocument();
    });
    expect(within(rowOf("U-100986")).getByRole("button", { name: "解禁" })).toBeInTheDocument();
    expect(mockState.audit[0]?.action).toBe("user.disable");
  });

  it("行操作打到数字主键上，展示号不进 URL", async () => {
    signIn();
    // 后端路径参数走 strconv.ParseInt，展示号会 400。
    // 这里直接断言实际请求路径，防止「从 U-100986 里 slice 出主键」的写法回归。
    const paths: string[] = [];
    const listener = ({ request }: { request: Request }) => {
      paths.push(`${request.method} ${new URL(request.url).pathname}`);
    };
    server.events.on("request:start", listener);

    const { user } = renderApp("/users");

    await screen.findByRole("cell", { name: "U-100986" });
    await user.click(within(rowOf("U-100986")).getByRole("button", { name: "禁用" }));
    const dialog = await screen.findByRole("dialog");
    await user.click(within(dialog).getByRole("button", { name: "禁用" }));
    await screen.findByText("已禁用 U-100986。");

    expect(paths).toContain("POST /api/admin/v1/users/100986/disable");
    expect(paths.some((path) => path.includes("U-100986"))).toBe(false);
    server.events.removeListener("request:start", listener);
  });

  it("确认框可用 Esc 关闭且不执行操作", async () => {
    signIn();
    const { user } = renderApp("/users");

    await screen.findByRole("cell", { name: "U-100986" });
    await user.click(within(rowOf("U-100986")).getByRole("button", { name: "禁用" }));
    await screen.findByRole("dialog");

    await user.keyboard("{Escape}");

    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(within(rowOf("U-100986")).getByText("已订阅")).toBeInTheDocument();
  });

  it("已禁用的用户显示「解禁」，解禁后状态回落", async () => {
    signIn();
    const { user } = renderApp("/users");

    await screen.findByRole("cell", { name: "U-100980" });
    await user.click(within(rowOf("U-100980")).getByRole("button", { name: "解禁" }));

    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).queryByText("该用户将立即被登出且无法登录。")).not.toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "解禁" }));

    expect(await screen.findByText("已解禁 U-100980。")).toBeInTheDocument();
    await waitFor(() => {
      expect(within(rowOf("U-100980")).getByText("未订阅")).toBeInTheDocument();
    });
  });

  it("support 的禁用入口是禁用态并说明原因", async () => {
    signInAs("support");
    renderApp("/users");

    await screen.findByRole("cell", { name: "U-100986" });
    const disableButton = within(rowOf("U-100986")).getByRole("button", { name: "禁用" });

    expect(disableButton).toBeDisabled();
    // 禁用的按钮不可聚焦，原因只能靠 aria-describedby 传达。
    expect(disableButton).toHaveAccessibleDescription(
      "禁用与解禁会立即把用户挡在门外，仅超级管理员与运营可执行。",
    );
    // 只读能力不受影响：列表本身照常可用。
    expect(screen.getByText("m***y@example.com")).toBeInTheDocument();
  });

  it.each(["owner", "ops"] as const)("%s 可以禁用用户", async (role) => {
    signInAs(role);
    renderApp("/users");

    await screen.findByRole("cell", { name: "U-100986" });
    expect(within(rowOf("U-100986")).getByRole("button", { name: "禁用" })).toBeEnabled();
    expect(
      screen.queryByText("禁用与解禁会立即把用户挡在门外，仅超级管理员与运营可执行。"),
    ).not.toBeInTheDocument();
  });

  it("禁用被后端 403 挡下时就地说明，不踢回登录页", async () => {
    // 前端矩阵与后端不一致（或权限刚被回收）时的兜底：403 不等于会话失效。
    signInAs("owner");
    server.use(
      http.post("/api/admin/v1/users/:id/:action", () =>
        HttpResponse.json({ error: { code: "forbidden", message: "没有访问权限。" } }, { status: 403 }),
      ),
    );

    const { user } = renderApp("/users");
    await screen.findByRole("cell", { name: "U-100986" });
    await user.click(within(rowOf("U-100986")).getByRole("button", { name: "禁用" }));
    await user.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "禁用" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "禁用与解禁会立即把用户挡在门外，仅超级管理员与运营可执行。",
    );
    // 仍在用户页：侧栏与列表都在，没有换成整屏 403，也没有回到登录页。
    expect(screen.getByRole("navigation")).toBeInTheDocument();
    expect(screen.getByRole("cell", { name: "U-100986" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "CC避风港 运营后台" })).not.toBeInTheDocument();
  });

  it("加载失败显示错误条 + 重试", async () => {
    signIn();
    let attempt = 0;
    server.use(
      http.get("/api/admin/v1/users", () => {
        attempt += 1;
        if (attempt === 1) {
          return HttpResponse.json(
            { error: { code: "internal_error", message: "服务暂时不可用，请稍后重试。" } },
            { status: 500 },
          );
        }
        return HttpResponse.json({
          data: { items: mockState.users, total: mockState.users.length, page: 1, page_size: 20 },
        });
      }),
    );

    const { user } = renderApp("/users");

    expect(await screen.findByRole("alert")).toHaveTextContent("用户数据加载失败：");
    await user.click(screen.getByRole("button", { name: "重试" }));
    expect(await screen.findByRole("cell", { name: "U-100986" })).toBeInTheDocument();
  });
});
