import { describe, expect, it } from "vitest";
import { screen } from "@testing-library/react";
import { http, HttpResponse } from "msw";
import { server } from "../mocks/server";
import { renderApp, signIn } from "./utils";

const forbidden = () =>
  HttpResponse.json({ error: { code: "forbidden", message: "没有访问权限。" } }, { status: 403 });

describe("403 权限不足", () => {
  it("非管理员会话进入后台时显示 403 页", async () => {
    signIn();
    server.use(http.get("/api/admin/v1/auth/me", forbidden));

    renderApp();

    expect(await screen.findByRole("heading", { name: "403 — 没有访问权限" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "仪表盘" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "换个账号登录" })).toBeInTheDocument();
  });

  it("业务接口返回 403 时同样落到 403 页", async () => {
    signIn();
    server.use(http.get("/api/admin/v1/users", forbidden));

    renderApp("/users");

    expect(await screen.findByRole("heading", { name: "403 — 没有访问权限" })).toBeInTheDocument();
  });

  it("403 页可以退回登录页", async () => {
    signIn();
    server.use(http.get("/api/admin/v1/auth/me", forbidden));

    const { user } = renderApp();
    await screen.findByRole("heading", { name: "403 — 没有访问权限" });

    await user.click(screen.getByRole("button", { name: "换个账号登录" }));
    expect(await screen.findByRole("heading", { name: "CC避风港 运营后台" })).toBeInTheDocument();
  });
});
