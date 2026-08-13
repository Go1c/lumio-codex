import { act, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { TerminalPane } from "./TerminalPane";
import { EVENTS } from "../lib/api";
import { renderWithProviders } from "../test/render";

describe("终端（5.5）", () => {
  it("连接建立前显示居中 spinner，就绪后消失", async () => {
    renderWithProviders(<TerminalPane projectId="project-1" host="root@43.156.20.8" />);

    expect(screen.getByText("正在连接 root@43.156.20.8…")).toBeInTheDocument();
    expect(await screen.findByTestId("terminal-host")).toBeInTheDocument();
  });

  it("断开时浮出重连横幅，可立即重连", async () => {
    const harness = renderWithProviders(
      <TerminalPane projectId="project-1" host="root@43.156.20.8" />,
    );
    await screen.findByTestId("terminal-host");

    act(() => harness.api.emit(EVENTS.terminalClosed("project-1"), null));

    // 第一次退避是 2 秒（6.3）。
    expect(await screen.findByText("连接已断开，2 秒后自动重连…")).toBeInTheDocument();

    await harness.user.click(screen.getByRole("button", { name: "立即重连" }));
    expect(harness.api.calls).toContain("closeTerminal");
  });
});
