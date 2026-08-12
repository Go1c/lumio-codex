import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { FilesExplorer } from "./FilesExplorer";
import { renderWithProviders } from "../test/render";
import { sampleConflicts, sampleFiles } from "../lib/mockApi";
import type { MockOptions } from "../lib/mockApi";

function setup(options: MockOptions = {}, conflictPaths: string[] = []) {
  const onGoToConflicts = vi.fn();
  const harness = renderWithProviders(
    <FilesExplorer
      projectId="project-1"
      projectName="my-project"
      conflictPaths={conflictPaths}
      onGoToConflicts={onGoToConflicts}
    />,
    { files: sampleFiles(), ...options },
  );
  return { ...harness, onGoToConflicts };
}

/** Scope queries to the explorer tree: the recent panel repeats file paths. */
function tree() {
  return within(screen.getByTestId("file-tree"));
}

describe("资源管理器（5.5 文件）", () => {
  it("先显示骨架，再列出文件夹在前的目录树", async () => {
    setup();
    expect(screen.getByTestId("files-skeleton")).toBeInTheDocument();

    expect(await screen.findByText("src")).toBeInTheDocument();
    const rows = screen.getAllByRole("button").map((node) => node.textContent ?? "");
    expect(rows.findIndex((text) => text.includes("src"))).toBeLessThan(
      rows.findIndex((text) => text.includes("Cargo.toml")),
    );
  });

  it("无打开文件时显示「最近更新」面板，点击卡片打开文件", async () => {
    const harness = setup();
    expect(await screen.findByText("最近更新")).toBeInTheDocument();

    await harness.user.click(await screen.findByText("src/engine.rs"));

    // 打开后出现标签页、面包屑与带行号的代码视图。
    expect(await screen.findByRole("tab", { name: /engine\.rs/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "复制路径" })).toBeInTheDocument();
    expect(screen.getByText("pub struct WriteBatcher {")).toBeInTheDocument();
  });

  it("行内新建文件：Enter 确认并提示正在同步", async () => {
    const harness = setup();
    await screen.findByText("src");

    await harness.user.click(screen.getByRole("button", { name: "新建文件" }));
    const input = screen.getByRole("textbox");
    await harness.user.type(input, "notes.md{Enter}");

    expect(await screen.findByText("已创建 notes.md，正在同步到服务器…")).toBeInTheDocument();
    await waitFor(() => expect(tree().getByText("notes.md")).toBeInTheDocument());
  });

  it("行内新建可用 Esc 取消，不创建任何文件", async () => {
    const harness = setup();
    await screen.findByText("src");

    await harness.user.click(screen.getByRole("button", { name: "新建文件夹" }));
    await harness.user.type(screen.getByRole("textbox"), "scratch{Escape}");

    await waitFor(() => expect(screen.queryByRole("textbox")).not.toBeInTheDocument());
    expect(tree().queryByText("scratch")).not.toBeInTheDocument();
    expect(harness.api.calls).not.toContain("createEntry");
  });

  it("右键菜单可重命名，改名后同步提示带新名字", async () => {
    const harness = setup();
    await screen.findByText("src");
    const row = tree().getByText("README.md");

    fireEvent.contextMenu(row);
    await harness.user.click(await screen.findByRole("menuitem", { name: "重命名" }));
    const input = screen.getByRole("textbox");
    await harness.user.clear(input);
    await harness.user.type(input, "GUIDE.md{Enter}");

    expect(await screen.findByText("已重命名为 GUIDE.md，正在同步到服务器…")).toBeInTheDocument();
    await waitFor(() => expect(tree().getByText("GUIDE.md")).toBeInTheDocument());
  });

  it("删除是两端同步的，并在 10 秒内可撤销", async () => {
    const harness = setup();
    await screen.findByText("src");
    const row = tree().getByText("README.md");

    fireEvent.contextMenu(row);
    await harness.user.click(await screen.findByRole("menuitem", { name: "删除" }));

    expect(await screen.findByText("已删除 README.md（两端同步删除）。")).toBeInTheDocument();
    await waitFor(() => expect(tree().queryByText("README.md")).not.toBeInTheDocument());

    await harness.user.click(screen.getByRole("button", { name: "撤销" }));
    await waitFor(() => expect(tree().getByText("README.md")).toBeInTheDocument());
    expect(await screen.findByText("已撤销删除 README.md。")).toBeInTheDocument();
  });

  it("空白处右键提供新建与刷新", async () => {
    const harness = setup();
    await screen.findByText("src");

    fireEvent.contextMenu(screen.getByText("资源管理器"));
    expect(await screen.findByRole("menuitem", { name: "新建文件" })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: "新建文件夹" })).toBeInTheDocument();

    await harness.user.click(screen.getByRole("menuitem", { name: "刷新" }));
    expect(await screen.findByText("已刷新，与服务器一致。")).toBeInTheDocument();
  });

  it("冲突文件的标签页带 ⚠ 并可直达冲突页", async () => {
    const harness = setup({ conflicts: sampleConflicts() }, ["src/engine.rs"]);
    await harness.user.click(await screen.findByText("src/engine.rs"));

    const tab = await screen.findByRole("tab", { name: /engine\.rs/ });
    expect(tab.textContent).toContain("⚠");

    await harness.user.click(screen.getByRole("button", { name: "去「冲突」页处理 →" }));
    expect(harness.onGoToConflicts).toHaveBeenCalled();
  });

  it("超过 1MB 的文件不预览", async () => {
    const harness = setup({
      files: [{ name: "big.log", path: "big.log", kind: "file", size: 2_000_000, modifiedMs: 1 }],
    });
    await screen.findByTestId("file-tree");
    await harness.user.click(tree().getByText("big.log"));
    expect(await screen.findByText("文件过大，无法预览。")).toBeInTheDocument();
  });
});
