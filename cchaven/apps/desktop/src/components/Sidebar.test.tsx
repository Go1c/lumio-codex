import { describe, expect, it } from "vitest";
import { aggregateStatus, syncBarLabel } from "./Sidebar";
import { sampleProject } from "../lib/mockApi";
import type { SyncStatus } from "../lib/types";

const projects = [sampleProject({ id: "a" }), sampleProject({ id: "b" })];

function statuses(a: SyncStatus, b: SyncStatus): Record<string, SyncStatus> {
  return { a, b };
}

const synced: SyncStatus = { state: "synced", conflicts: 0, pending: 0 };

describe("全局同步状态（6.3）", () => {
  it("文案与状态一一对应", () => {
    expect(syncBarLabel(synced)).toBe("已全部同步");
    expect(syncBarLabel({ state: "syncing", conflicts: 0, pending: 3 })).toBe(
      "正在同步 3 个文件…",
    );
    expect(syncBarLabel({ state: "conflicts", conflicts: 2, pending: 0 })).toBe("2 个冲突");
    expect(
      syncBarLabel({ state: "offline", conflicts: 0, pending: 0, retryInSeconds: 5 }, 5),
    ).toBe(
      "离线 — 5 秒后重试",
    );
  });

  it("聚合时冲突优先于离线，离线优先于同步中", () => {
    expect(
      aggregateStatus(
        projects,
        statuses({ state: "conflicts", conflicts: 2, pending: 0 }, { state: "offline", conflicts: 0, pending: 0 }),
        false,
      ).state,
    ).toBe("conflicts");

    expect(
      aggregateStatus(
        projects,
        statuses({ state: "syncing", conflicts: 0, pending: 3 }, { state: "offline", conflicts: 0, pending: 0 }),
        false,
      ).state,
    ).toBe("offline");

    expect(
      aggregateStatus(projects, statuses({ state: "syncing", conflicts: 0, pending: 3 }, synced), false)
        .state,
    ).toBe("syncing");

    expect(aggregateStatus(projects, statuses(synced, synced), false).state).toBe("synced");
    expect(aggregateStatus(projects, statuses(synced, synced), true).state).toBe("offline");
  });

  it("冲突计数为所有项目之和", () => {
    const total = aggregateStatus(
      projects,
      statuses(
        { state: "conflicts", conflicts: 2, pending: 0 },
        { state: "conflicts", conflicts: 1, pending: 0 },
      ),
      false,
    );
    expect(total.conflicts).toBe(3);
    expect(syncBarLabel(total)).toBe("3 个冲突");
  });
});
