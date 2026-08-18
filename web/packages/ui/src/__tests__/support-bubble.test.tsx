import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { SupportBubble } from "../components/SupportBubble";

const QQ_NUMBER = "1073671738";
const FEISHU_URL =
  "https://applink.feishu.cn/client/chat/chatter/add_by_link?link_token=test-feishu";

function stubClipboard(writeText: (text: string) => Promise<void>): void {
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { writeText },
  });
}

beforeEach(() => {
  vi.stubEnv("VITE_SUPPORT_QQ_NUMBER", QQ_NUMBER);
  vi.stubEnv("VITE_SUPPORT_FEISHU_URL", FEISHU_URL);
  stubClipboard(async () => undefined);
});

afterEach(() => {
  vi.unstubAllEnvs();
});

describe("SupportBubble", () => {
  it("右下角有客服气泡，默认不打开面板", () => {
    render(<SupportBubble />);

    expect(screen.getByRole("button", { name: "客服与反馈" })).toBeInTheDocument();
    expect(screen.queryByRole("dialog", { name: "Lumio 支持" })).not.toBeInTheDocument();
  });

  it("打开后面板展示 QQ 群号，飞书仍是外链", async () => {
    const user = userEvent.setup();
    render(<SupportBubble />);

    await user.click(screen.getByRole("button", { name: "客服与反馈" }));

    expect(screen.getByRole("dialog", { name: "Lumio 支持" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /QQ 群号 1073671738/ })).toBeInTheDocument();
    const feishu = screen.getByRole("link", { name: /飞书群/ });
    expect(feishu).toHaveAttribute("href", FEISHU_URL);
    expect(feishu).toHaveAttribute("target", "_blank");
    expect(feishu).toHaveAttribute("rel", "noreferrer");
    expect(screen.queryByRole("link", { name: /QQ/ })).not.toBeInTheDocument();
  });

  it("点击 QQ 群号复制到剪贴板", async () => {
    const written: string[] = [];
    stubClipboard(async (text) => {
      written.push(text);
    });
    render(<SupportBubble />);

    await userEvent.click(screen.getByRole("button", { name: "客服与反馈" }));
    await userEvent.click(screen.getByRole("button", { name: /QQ 群号 1073671738/ }));

    expect(written).toEqual([QQ_NUMBER]);
    expect(await screen.findByText("已复制到剪贴板")).toBeInTheDocument();
  });

  it("再点气泡或按 Esc 关闭面板", async () => {
    const user = userEvent.setup();
    render(<SupportBubble />);

    const launcher = screen.getByRole("button", { name: "客服与反馈" });
    await user.click(launcher);
    expect(screen.getByRole("dialog", { name: "Lumio 支持" })).toBeInTheDocument();

    await user.click(launcher);
    expect(screen.queryByRole("dialog", { name: "Lumio 支持" })).not.toBeInTheDocument();

    await user.click(launcher);
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog", { name: "Lumio 支持" })).not.toBeInTheDocument();
  });

  it("未配 QQ 群时只渲染飞书入口", async () => {
    vi.stubEnv("VITE_SUPPORT_QQ_NUMBER", "off");
    const user = userEvent.setup();
    render(<SupportBubble />);

    await user.click(screen.getByRole("button", { name: "客服与反馈" }));
    expect(screen.getByRole("link", { name: /飞书群/ })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /QQ 群号/ })).not.toBeInTheDocument();
  });

  it("两条社群入口都空时不渲染气泡", () => {
    vi.stubEnv("VITE_SUPPORT_QQ_NUMBER", "off");
    vi.stubEnv("VITE_SUPPORT_FEISHU_URL", "off");

    const { container } = render(<SupportBubble />);
    expect(container).toBeEmptyDOMElement();
  });
});
