import { screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { db } from "@/mocks/db";
import { renderApp } from "@/test/utils";

/** APP 授权页 `/authorize`（交互设计 3.4 / 5.1）。 */

const QUERY = new URLSearchParams({
  client_id: "cchaven-desktop",
  redirect_uri: "http://127.0.0.1:53682/callback",
  scope: "profile workspace offline_access",
  code_challenge: "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
  code_challenge_method: "S256",
  state: "xyz",
});

const ROUTE = `/authorize?${QUERY.toString()}`;

describe("授权页 /authorize", () => {
  it("参数非法时展示不可继续的错误态", async () => {
    renderApp("/authorize?client_id=unknown-app&code_challenge_method=S256");

    expect(await screen.findByRole("heading", { name: "无法完成授权" })).toBeInTheDocument();
    expect(screen.getByText("授权请求参数不正确。请回到 APP 重新发起登录。")).toBeInTheDocument();
    expect(screen.getByText("未知的 client_id")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "授权" })).not.toBeInTheDocument();
  });

  it("未登录时先展示请求方与权限，再引导登录并带回跳参数", async () => {
    renderApp(ROUTE);

    expect(
      await screen.findByRole("heading", { name: "授权 CC避风港 APP 访问你的账号" }),
    ).toBeInTheDocument();
    expect(screen.getByText("读取你的账号邮箱与订阅状态")).toBeInTheDocument();
    expect(screen.getByText("代表你连接与同步你的工作区")).toBeInTheDocument();
    expect(screen.getByText("请先登录后再继续授权。")).toBeInTheDocument();

    const loginLink = screen.getByRole("link", { name: "登录后继续授权" });
    expect(loginLink).toHaveAttribute("href", `/login?next=${encodeURIComponent(ROUTE)}`);
    expect(screen.queryByRole("button", { name: "授权" })).not.toBeInTheDocument();
  });

  it("已登录时展示当前账号与授权 / 取消按钮", async () => {
    db.loggedIn = true;
    renderApp(ROUTE);

    expect(await screen.findByText(/当前登录账号/)).toBeInTheDocument();
    expect(await screen.findByRole("button", { name: "授权" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "取消" })).toBeEnabled();
  });

  it("点授权后跳 redirect_to，并把授权码展示出来作为手动兜底", async () => {
    db.loggedIn = true;
    const assign = vi.fn();
    vi.spyOn(window, "location", "get").mockReturnValue({
      ...window.location,
      assign,
    } as unknown as Location);

    const { user } = renderApp(ROUTE);
    await user.click(await screen.findByRole("button", { name: "授权" }));

    expect(await screen.findByRole("heading", { name: "授权成功" })).toBeInTheDocument();
    expect(screen.getByText("mock-authorization-code-8f2a1c")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "复制授权码" })).toBeInTheDocument();
    expect(screen.getByText("授权码 5 分钟内有效，只能使用一次。")).toBeInTheDocument();

    expect(assign).toHaveBeenCalledWith(
      "http://127.0.0.1:53682/callback?code=mock-authorization-code-8f2a1c&state=xyz",
    );
    expect(screen.getByRole("link", { name: "重新打开 APP" })).toHaveAttribute(
      "href",
      "http://127.0.0.1:53682/callback?code=mock-authorization-code-8f2a1c&state=xyz",
    );
  });

  it("点取消后停在已取消提示，不发放授权码", async () => {
    db.loggedIn = true;
    const { user } = renderApp(ROUTE);

    await user.click(await screen.findByRole("button", { name: "取消" }));

    expect(await screen.findByText("已取消授权，你可以关闭此页面。")).toBeInTheDocument();
    expect(screen.queryByText("mock-authorization-code-8f2a1c")).not.toBeInTheDocument();
  });
});
