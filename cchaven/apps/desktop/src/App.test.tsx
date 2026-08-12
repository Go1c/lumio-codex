import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import App from "./App";
import { renderWithProviders } from "./test/render";
import { sampleProject } from "./lib/mockApi";

/** The sidebar and the main area both offer 「+ 新建项目」/project names. */
const sidebar = () => within(screen.getByRole("complementary"));
const main = () => within(screen.getByRole("main"));

describe("应用外壳（5.2 / 5.4）", () => {
  it("0 个项目时进入空状态页，而不是被强制拉进向导", async () => {
    renderWithProviders(<App />, { signedIn: true });

    expect(await screen.findByText("把你的云服务器变成 Claude Code 工作台")).toBeInTheDocument();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(screen.getByText("还没有项目。")).toBeInTheDocument();
  });

  it("向导以模态出现，取消后回到空状态页", async () => {
    const harness = renderWithProviders(<App />, { signedIn: true });
    await screen.findByText("把你的云服务器变成 Claude Code 工作台");

    await harness.user.click(main().getByRole("button", { name: "+ 新建项目" }));
    expect(await screen.findByRole("dialog")).toBeInTheDocument();

    await harness.user.click(screen.getByRole("button", { name: "取消" }));

    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(screen.getByText("把你的云服务器变成 Claude Code 工作台")).toBeInTheDocument();
  });

  it("侧栏列出项目并可进入工作区，顶部信息栏带服务器芯片", async () => {
    const harness = renderWithProviders(<App />, {
      signedIn: true,
      projects: [sampleProject()],
    });

    await screen.findByText("my-project");
    await harness.user.click(sidebar().getByRole("button", { name: "my-project" }));

    expect(await screen.findByText("🖥 root@43.156.20.8")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "打开本地文件夹" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "终端" })).toBeInTheDocument();
  });

  it("删除项目需二次确认，并说明不会删除任何文件", async () => {
    const harness = renderWithProviders(<App />, {
      signedIn: true,
      projects: [sampleProject()],
    });
    await screen.findByText("my-project");

    await harness.user.click(sidebar().getByRole("button", { name: "my-project 的更多操作" }));
    await harness.user.click(await screen.findByRole("menuitem", { name: "删除…" }));

    expect(
      await screen.findByText(
        "该项目将从 CC避风港 中移除。不会删除任何本地或远端文件。",
      ),
    ).toBeInTheDocument();

    await harness.user.click(screen.getByRole("button", { name: "移除项目" }));
    expect(await screen.findByText("已移除项目「my-project」。")).toBeInTheDocument();
    await waitFor(() =>
      expect(sidebar().queryByRole("button", { name: "my-project" })).not.toBeInTheDocument(),
    );
  });

  it("订阅剩余 ≤3 天时顶部出现一次性续费横幅", async () => {
    const harness = renderWithProviders(<App />, { signedIn: true, daysLeft: 2 });

    expect(await screen.findByText("订阅即将到期，去官网续费 ↗")).toBeInTheDocument();
    await harness.user.click(screen.getByRole("button", { name: "关闭" }));
    await waitFor(() =>
      expect(screen.queryByText("订阅即将到期，去官网续费 ↗")).not.toBeInTheDocument(),
    );
  });

  it("网络不可达但本地有缓存项目时进入离线只读模式", async () => {
    renderWithProviders(<App />, {
      signedIn: true,
      offline: true,
      projects: [sampleProject()],
    });

    expect(
      await screen.findByText("当前处于离线只读模式，终端与同步已暂停。"),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "重试连接" })).toBeInTheDocument();
  });

  it("未登录且无缓存项目时停在登录页", async () => {
    renderWithProviders(<App />, { offline: true });
    expect(await screen.findByRole("button", { name: "通过浏览器登录 ↗" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "离线使用" })).not.toBeInTheDocument();
  });
});
