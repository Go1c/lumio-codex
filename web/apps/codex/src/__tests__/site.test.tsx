import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";

import { siteUrl } from "@lumio/ui";

import { App } from "@/App";

const MANIFEST = {
  version: "1.2.46",
  assets: [
    {
      name: "LumioCodex-1.2.46-macos-arm64-internal-unsigned.dmg",
      url: "https://s3.example.com/arm64.dmg",
    },
    {
      name: "LumioCodex-1.2.46-windows-x64-setup-internal-unsigned.exe",
      url: "https://s3.example.com/setup.exe",
    },
  ],
};

function renderApp(path = "/") {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <App />
    </MemoryRouter>,
  );
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("Codex 产品站", () => {
  it("品牌是 BestCodex，顶栏 Codex/Claude 是指向另一站的整页链接", () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.reject(new Error("offline"))),
    );

    renderApp();

    expect(screen.getByRole("link", { name: /^BestCodex$/ })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Codex" })).toHaveAttribute("href", siteUrl("codex"));
    expect(screen.getByRole("link", { name: "Claude" })).toHaveAttribute("href", siteUrl("cc"));
    expect(document.querySelector("[data-pane]")).toBeNull();
  });

  it("首页讲清「不是官方应用」的定位，且没有禁止文案", () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.reject(new Error("offline"))),
    );

    renderApp();

    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("更快开始使用官方 Codex");
    expect(screen.getByText(/你使用的始终是官方 Codex 应用/)).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "三步开始" })).toBeInTheDocument();
    expect(screen.queryByText(/向下滚动/)).not.toBeInTheDocument();
    expect(screen.queryByText(/你好 Mary/)).not.toBeInTheDocument();
    expect(screen.queryByText(/已就绪/)).not.toBeInTheDocument();
    expect(screen.queryByText(/在哪里注册和充值/)).not.toBeInTheDocument();
    expect(screen.queryByText(/当前为内测渠道/)).not.toBeInTheDocument();
    const start = screen.getByRole("heading", { name: "三步开始" });
    expect(start.nextElementSibling?.textContent ?? "").not.toMatch(/官方 Codex 需单独安装/);
  });

  it("下载区是三平台，没有长说明", () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.reject(new Error("offline"))),
    );

    renderApp();

    expect(screen.getByRole("article", { name: "Mac · Apple 芯片" })).toBeInTheDocument();
    expect(screen.getByRole("article", { name: "Mac · Intel" })).toBeInTheDocument();
    expect(screen.getByRole("article", { name: "Windows" })).toBeInTheDocument();
    expect(screen.queryByText(/从浏览器下载后 macOS 可能提示/)).not.toBeInTheDocument();
  });

  it("页脚声明与 OpenAI、Anthropic 无从属", () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.reject(new Error("offline"))),
    );

    renderApp();

    expect(screen.getByText(/与 OpenAI、Anthropic 无从属/)).toBeInTheDocument();
  });

  it("带 Intel 字样的 Mac UA 把「你的设备」标在 Apple 芯片卡上", () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.reject(new Error("offline"))),
    );
    vi.spyOn(window.navigator, "userAgent", "get").mockReturnValue(
      "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15",
    );

    renderApp();

    expect(
      within(screen.getByRole("article", { name: "Mac · Apple 芯片" })).getByText("你的设备"),
    ).toBeInTheDocument();
    expect(
      within(screen.getByRole("article", { name: "Mac · Intel" })).queryByText("你的设备"),
    ).not.toBeInTheDocument();
  });

  it("按平台给出安装包，并在下载前弹确认层", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.resolve(new Response(JSON.stringify(MANIFEST), { status: 200 }))),
    );

    renderApp();

    const card = screen.getByRole("article", { name: "Mac · Apple 芯片" });
    await waitFor(() => expect(within(card).getByText(/v1.2.46/)).toBeInTheDocument());

    await userEvent.click(within(card).getByRole("button", { name: "下载" }));

    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByRole("link", { name: "开始下载" })).toHaveAttribute(
      "href",
      "https://s3.example.com/arm64.dmg",
    );
    expect(within(dialog).getByText(/xattr -cr/)).toBeInTheDocument();
  });

  it("清单里没有该平台的包时指向 GitHub 发布页", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() =>
        Promise.resolve(new Response(JSON.stringify({ version: "1.2.46", assets: [] }), { status: 200 })),
      ),
    );

    renderApp();

    const card = screen.getByRole("article", { name: "Windows" });
    await waitFor(() => expect(within(card).getByText(/GitHub/)).toBeInTheDocument());

    await userEvent.click(within(card).getByRole("button", { name: "下载" }));

    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByRole("link", { name: "前往发布页" })).toHaveAttribute(
      "href",
      "https://github.com/Go1c/lumio-codex/releases",
    );
  });

  it("CDN 不可用时三张卡都回退 GitHub，不留死链", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.reject(new Error("offline"))),
    );

    renderApp();

    await waitFor(() =>
      expect(screen.getAllByText(/CDN 暂不可用/).length).toBeGreaterThanOrEqual(3),
    );
  });

  it("账号入口回门户并带 next", () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.reject(new Error("offline"))),
    );

    renderApp();

    expect(screen.getByRole("link", { name: "登录" }).getAttribute("href")).toMatch(
      /^https:\/\/bestcodex\.app\/login\?next=/,
    );
  });
});

describe("帮助中心", () => {
  it("至少能渲染 5 个主题，并注明规范 URL", () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.reject(new Error("offline"))),
    );

    renderApp("/help");

    expect(screen.getByRole("heading", { name: /需要什么帮助/ })).toBeInTheDocument();
    const topics = screen.getByRole("navigation", { name: "帮助主题" });
    expect(within(topics).getByRole("link", { name: /安装/ })).toBeInTheDocument();
    expect(within(topics).getByRole("link", { name: /未签名/ })).toBeInTheDocument();
    expect(within(topics).getByRole("link", { name: /登录/ })).toBeInTheDocument();
    expect(within(topics).getByRole("link", { name: /修复/ })).toBeInTheDocument();
    expect(within(topics).getByRole("link", { name: /Claude 连服务器/ })).toBeInTheDocument();
    expect(screen.getByText("https://bestcodex.app/help")).toBeInTheDocument();
    expect(screen.getByText("https://codex.bestcodex.app/help")).toBeInTheDocument();
  });
});
