import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";

import { Modal } from "../components/Modal";
import { ToastProvider, useToast } from "../components/Toast";
import { TextField } from "../components/fields";
import { Banner, EmptyBlock, ErrorBlock, LoadingBlock, StatusDot } from "../components/ui";

describe("Banner", () => {
  it("错误用 alert 角色抢读，其余用 status 不打断", () => {
    const { rerender } = render(<Banner kind="error">出错了</Banner>);
    expect(screen.getByRole("alert")).toHaveTextContent("出错了");

    rerender(<Banner kind="ok">成功了</Banner>);
    expect(screen.getByRole("status")).toHaveTextContent("成功了");
  });
});

describe("ErrorBlock", () => {
  it("展示原因并提供重试动作，不留死胡同", async () => {
    const onRetry = vi.fn();
    render(<ErrorBlock message="服务暂时不可用" onRetry={onRetry} />);

    expect(screen.getByRole("alert")).toHaveTextContent("服务暂时不可用");
    await userEvent.click(screen.getByRole("button", { name: "重试" }));
    expect(onRetry).toHaveBeenCalledTimes(1);
  });
});

describe("LoadingBlock / EmptyBlock / StatusDot", () => {
  it("加载态对读屏可见", () => {
    render(<LoadingBlock label="加载账户…" />);
    expect(screen.getByRole("status")).toHaveTextContent("加载账户…");
  });

  it("空态给出说明而非空白", () => {
    render(<EmptyBlock text="暂无可下载的版本" />);
    expect(screen.getByText("暂无可下载的版本")).toBeInTheDocument();
  });

  it("状态点颜色之外必须有文字", () => {
    render(<StatusDot tone="green" label="正常" />);
    expect(screen.getByText("正常")).toBeInTheDocument();
  });
});

describe("TextField", () => {
  it("字段错误与提示通过 aria-describedby 关联输入框", () => {
    render(<TextField label="邮箱" error="请输入有效的邮箱地址。" hint="用于登录" />);

    const input = screen.getByLabelText("邮箱");
    expect(input).toHaveAttribute("aria-invalid", "true");
    const describedBy = input.getAttribute("aria-describedby") ?? "";
    expect(describedBy.split(" ")).toHaveLength(2);
    expect(screen.getByText("请输入有效的邮箱地址。")).toBeInTheDocument();
  });
});

describe("Modal", () => {
  it("Esc 关闭", async () => {
    const onClose = vi.fn();
    render(
      <Modal title="确认下载" onClose={onClose}>
        正文
      </Modal>,
    );

    expect(screen.getByRole("dialog")).toHaveAccessibleName("确认下载");
    await userEvent.keyboard("{Escape}");
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});

describe("Toast", () => {
  it("展示一次性通知", async () => {
    function Trigger() {
      const toast = useToast();
      const [n] = useState(0);
      return (
        <button type="button" onClick={() => toast(`已复制 ${n}`)}>
          复制
        </button>
      );
    }

    render(
      <ToastProvider>
        <Trigger />
      </ToastProvider>,
    );

    await userEvent.click(screen.getByRole("button", { name: "复制" }));
    expect(await screen.findByText("已复制 0")).toBeInTheDocument();
  });
});
