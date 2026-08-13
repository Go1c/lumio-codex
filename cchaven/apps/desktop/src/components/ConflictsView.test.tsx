import { screen, waitFor } from "@testing-library/react";
import { useCallback, useEffect, useState } from "react";
import { describe, expect, it } from "vitest";
import { ConflictsView } from "./ConflictsView";
import { renderWithProviders } from "../test/render";
import { MockApi, sampleConflicts } from "../lib/mockApi";
import type { Conflict } from "../lib/types";

/** Mirrors how Workspace owns the list and reloads it after each resolution. */
function Host({ api }: { api: MockApi }) {
  const [conflicts, setConflicts] = useState<Conflict[]>([]);
  const reload = useCallback(async () => {
    setConflicts(await api.listConflicts());
  }, [api]);
  useEffect(() => {
    void reload();
  }, [reload]);
  return <ConflictsView projectId="project-1" conflicts={conflicts} onChanged={reload} />;
}

function setup() {
  const api = new MockApi({ conflicts: sampleConflicts() });
  return renderWithProviders(<Host api={api} />, { api });
}

describe("冲突页（5.5 冲突）", () => {
  it("列出冲突文件与类型，并并排显示本地/远端", async () => {
    setup();

    expect(await screen.findByText("⚠ src/engine.rs")).toBeInTheDocument();
    expect(screen.getByText(/本地与远端同时修改/)).toBeInTheDocument();
    expect(screen.getByText(/^本地 —/)).toBeInTheDocument();
    expect(screen.getByText(/^远端 —/)).toBeInTheDocument();
  });

  it("三个解决按钮齐备，解决后列表递减且 10 秒内可撤销", async () => {
    const harness = setup();
    await screen.findByText("⚠ src/engine.rs");

    expect(screen.getByRole("button", { name: "保留本地" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "两者都保留（另存副本）" })).toBeInTheDocument();

    await harness.user.click(screen.getByRole("button", { name: "保留远端" }));

    expect(await screen.findByText("已解决 src/engine.rs — 保留远端。")).toBeInTheDocument();
    await waitFor(() => expect(screen.queryByText("⚠ src/engine.rs")).not.toBeInTheDocument());

    await harness.user.click(screen.getByRole("button", { name: "撤销" }));
    expect(await screen.findByText("⚠ src/engine.rs")).toBeInTheDocument();
    expect(
      await screen.findByText("已撤销，src/engine.rs 重新回到冲突列表。"),
    ).toBeInTheDocument();
  });

  it("「全部按…解决」批量处理后显示空状态", async () => {
    const harness = setup();
    await screen.findByText("⚠ src/engine.rs");

    await harness.user.selectOptions(screen.getByLabelText("全部按…解决"), "keepLocal");

    expect(await screen.findByText("没有冲突，已全部同步 ✓")).toBeInTheDocument();
  });

  it("远端已删除的冲突在远端栏说明情况", async () => {
    const harness = setup();
    await harness.user.click(await screen.findByText("⚠ Cargo.toml"));
    expect(await screen.findByText("远端已删除该文件")).toBeInTheDocument();
  });
});
