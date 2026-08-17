import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";

import { App } from "@/App";

const MANIFEST = {
  version: "1.2.46",
  assets: [
    {
      name: "LumioCodex-1.2.46-macos-arm64-internal-unsigned.dmg",
      url: "https://s3.example.com/arm64.dmg",
    },
    {
      name: "LumioCodex-1.2.46-macos-x64-internal-unsigned.dmg",
      url: "https://s3.example.com/x64.dmg",
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
  vi.unstubAllEnvs();
  vi.unstubAllGlobals();
});

describe("BestCodex 单站", () => {
  it("默认 / 渲染 Codex 落地页", () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.reject(new Error("offline"))),
    );

    renderApp("/");

    expect(screen.getByRole("link", { name: /^BestCodex$/ })).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("更快开始使用官方 Codex");
    expect(screen.getByRole("heading", { name: "三步开始" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Codex" })).toHaveAttribute("aria-current", "page");
    expect(document.querySelector("[data-pane]")).toBeNull();
    expect(screen.queryByText(/你好 Mary/)).not.toBeInTheDocument();
  });

  it("顶栏 Claude 是站内路由，点击后换页且不整页跳转", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.reject(new Error("offline"))),
    );

    renderApp("/");

    const claude = screen.getByRole("link", { name: "Claude" });
    expect(claude).toHaveAttribute("href", "/claude");
    expect(screen.getByRole("link", { name: "Codex" })).toHaveAttribute("href", "/codex");

    await userEvent.click(claude);

    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("安心使用 Claude Code");
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("不再担心封号");
    expect(screen.getByRole("link", { name: "Claude" })).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("link", { name: "Codex" })).not.toHaveAttribute("aria-current");
  });

  it("Claude 页在下载区之后挂 FAQ", () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.reject(new Error("offline"))),
    );

    renderApp("/claude");

    const downloads = document.getElementById("downloads");
    const faqQuestion = screen.getByRole("button", { name: /有没有免费版/ });
    expect(downloads).toBeTruthy();
    expect(faqQuestion).toBeInTheDocument();
    expect(
      downloads!.compareDocumentPosition(faqQuestion) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
  });

  it("Claude 装饰终端不含 agent / tmux", () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.reject(new Error("offline"))),
    );

    renderApp("/claude");

    expect(document.body.textContent).not.toMatch(/\btmux\b/i);
    expect(document.body.textContent).not.toMatch(/\bagent\b/i);
    expect(screen.getByText(/attached\s+session my-project/)).toBeInTheDocument();
  });

  it("/pricing 站内落到 Claude 页定价锚点", () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.reject(new Error("offline"))),
    );

    renderApp("/pricing");

    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("安心使用 Claude Code");
    expect(document.getElementById("pricing")).toBeTruthy();
    expect(screen.getByText("¥19.9")).toBeInTheDocument();
  });

  it("/download 站内落到共享下载区", () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.reject(new Error("offline"))),
    );

    renderApp("/download");

    expect(document.getElementById("downloads")).toBeTruthy();
    expect(screen.getByRole("article", { name: "Mac · Apple 芯片" })).toBeInTheDocument();
  });

  it("/codex 与首页一样是 Codex 落地页", () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.reject(new Error("offline"))),
    );

    renderApp("/codex");

    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("更快开始使用官方 Codex");
    expect(screen.getByRole("heading", { name: "三步开始" })).toBeInTheDocument();
  });
});

describe("Codex 页内容", () => {
  it("讲清「不是官方应用」的定位，且没有禁止文案", () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.reject(new Error("offline"))),
    );

    renderApp("/");

    expect(screen.getByText(/你使用的始终是官方 Codex 应用/)).toBeInTheDocument();
    expect(screen.queryByText(/向下滚动/)).not.toBeInTheDocument();
    expect(screen.queryByText(/已就绪/)).not.toBeInTheDocument();
    expect(screen.queryByText(/在哪里注册和充值/)).not.toBeInTheDocument();
    expect(screen.queryByText(/当前为内测渠道/)).not.toBeInTheDocument();
    const start = screen.getByRole("heading", { name: "三步开始" });
    expect(start.nextElementSibling?.textContent ?? "").not.toMatch(/官方 Codex 需单独安装/);
  });

  it("FAQ 说明 macOS 提示已损坏时怎么打开", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.reject(new Error("offline"))),
    );

    renderApp("/");

    const question = screen.getByRole("button", { name: /已损坏，无法打开/ });
    expect(question).toHaveAttribute("aria-expanded", "false");

    await userEvent.click(question);

    expect(question).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByText(/xattr -cr "\/Applications\/BestCodex.app"/)).toBeInTheDocument();
    expect(screen.queryByText(/Lumio Codex\.app/)).not.toBeInTheDocument();
  });

  it("下载区是三平台，没有长说明", () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.reject(new Error("offline"))),
    );

    renderApp("/");

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

    renderApp("/");

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

describe("Claude 页内容", () => {
  it("讲清防封与双向同步，且没有禁止文案", () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.reject(new Error("offline"))),
    );

    renderApp("/claude");

    expect(screen.getByRole("heading", { name: "防封方案" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "双向安全同步" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "防封，以及同步" })).toBeInTheDocument();
    expect(screen.queryByText(/向下滚动/)).not.toBeInTheDocument();
    expect(screen.queryByText(/主因是防封/)).not.toBeInTheDocument();
    expect(screen.queryByText(/一个价钱，功能全开/)).not.toBeInTheDocument();
    expect(screen.queryByText(/把 Claude Code 放到自己的服务器上/)).not.toBeInTheDocument();
    expect(screen.queryByText(/把 Claude Code 搬进避风港/)).not.toBeInTheDocument();
    expect(screen.queryByText(/你好 Mary/)).not.toBeInTheDocument();
  });

  it("简单定价 ¥19.9，邀请两行在订阅按钮下面", () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.reject(new Error("offline"))),
    );

    renderApp("/claude");

    expect(screen.getByRole("heading", { name: "简单定价" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Claude 包月" })).toBeInTheDocument();
    expect(screen.getByText("¥19.9")).toBeInTheDocument();
    expect(screen.getByText(/\/ 月/)).toBeInTheDocument();

    const subscribe = screen.getByRole("link", { name: "立即订阅" });
    expect(subscribe).toHaveAttribute("href", "https://api.lumio.games/purchase");

    const invite = screen.getByText(/经朋友邀请注册并登录 APP/);
    const once = screen.getByText("首月免费（每个账号限一次）");
    expect(subscribe.compareDocumentPosition(invite) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(invite.compareDocumentPosition(once) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(invite.textContent).not.toContain("首月免费（每个账号限一次）");
  });

  it("FAQ 手风琴同时只展开一项", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.reject(new Error("offline"))),
    );

    renderApp("/claude");

    const question = screen.getByRole("button", { name: /有没有免费版/ });
    expect(question).toHaveAttribute("aria-expanded", "false");

    await userEvent.click(question);

    expect(question).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByText(/免费试用一个月/)).toBeInTheDocument();
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
    expect(screen.queryByText("https://codex.bestcodex.app/help")).not.toBeInTheDocument();
  });

  it("顶栏帮助链到同一套内容", () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.reject(new Error("offline"))),
    );

    renderApp("/");
    expect(screen.getByRole("link", { name: "帮助" }).getAttribute("href")).toMatch(/\/help$/);
  });
});
