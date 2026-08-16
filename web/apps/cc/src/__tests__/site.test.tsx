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

describe("首页", () => {
  it("品牌是 BestCodex，顶栏 Codex/Claude 换整页", () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.reject(new Error("offline"))),
    );

    renderApp("/");

    expect(screen.getByRole("link", { name: /^BestCodex$/ })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Codex" })).toHaveAttribute("href", siteUrl("codex"));
    expect(screen.getByRole("link", { name: "Claude" })).toHaveAttribute("href", siteUrl("cc"));
    expect(document.querySelector("[data-pane]")).toBeNull();
  });

  it("讲清防封与双向同步，且没有禁止文案", () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.reject(new Error("offline"))),
    );

    renderApp("/");

    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("安心使用 Claude Code");
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("不再担心封号");
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

    renderApp("/");

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

  it("下载三平台，页脚无从属", () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.reject(new Error("offline"))),
    );

    renderApp("/");

    expect(screen.getByRole("article", { name: "Mac · Apple 芯片" })).toBeInTheDocument();
    expect(screen.getByRole("article", { name: "Mac · Intel" })).toBeInTheDocument();
    expect(screen.getByRole("article", { name: "Windows" })).toBeInTheDocument();
    expect(screen.getByText(/与 OpenAI、Anthropic 无从属/)).toBeInTheDocument();
  });
});

describe("定价页", () => {
  it("单一套餐 ¥19.9/月，订阅按钮直达 Sub2API 收银台", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.reject(new Error("offline"))),
    );

    renderApp("/pricing");

    expect(screen.getByText("¥19.9")).toBeInTheDocument();
    expect(screen.getByText(/首月免费（每个账号限一次）/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "立即订阅" })).toHaveAttribute(
      "href",
      "https://api.lumio.games/purchase",
    );
  });

  it("FAQ 手风琴同时只展开一项", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.reject(new Error("offline"))),
    );

    renderApp("/pricing");

    const question = screen.getByRole("button", { name: /有没有免费版/ });
    expect(question).toHaveAttribute("aria-expanded", "false");

    await userEvent.click(question);

    expect(question).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByText(/免费试用一个月/)).toBeInTheDocument();
  });
});

describe("下载页", () => {
  it("配置了清单时给出三平台安装包", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.resolve(new Response(JSON.stringify(MANIFEST), { status: 200 }))),
    );

    renderApp("/download");

    const card = screen.getByRole("article", { name: "Mac · Apple 芯片" });
    await waitFor(() => expect(within(card).getByText(/v1.2.46/)).toBeInTheDocument());
    expect(screen.getByRole("article", { name: "Mac · Intel" })).toBeInTheDocument();
    expect(screen.getByRole("article", { name: "Windows" })).toBeInTheDocument();
  });
});

describe("账号入口", () => {
  it("产品站不做登录，账号链接一律回门户并带 next", () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.reject(new Error("offline"))),
    );

    renderApp("/pricing");

    const login = screen.getByRole("link", { name: "登录" });
    expect(login.getAttribute("href")).toMatch(/^https:\/\/bestcodex\.app\/login\?next=/);
    expect(screen.getByRole("link", { name: "注册" }).getAttribute("href")).toMatch(
      /^https:\/\/bestcodex\.app\/signup\?next=/,
    );
  });
});

describe("帮助", () => {
  it("顶栏帮助链到同一套内容，页面至少 5 个主题", () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.reject(new Error("offline"))),
    );

    renderApp("/");
    const help = screen.getByRole("link", { name: "帮助" });
    expect(help.getAttribute("href")).toMatch(/\/help$/);
  });

  it("帮助中心至少列出 5 个主题", () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.reject(new Error("offline"))),
    );

    renderApp("/help");
    const topics = screen.getByRole("navigation", { name: "帮助主题" });
    expect(within(topics).getByRole("link", { name: /安装/ })).toBeInTheDocument();
    expect(within(topics).getByRole("link", { name: /未签名/ })).toBeInTheDocument();
    expect(within(topics).getByRole("link", { name: /登录/ })).toBeInTheDocument();
    expect(within(topics).getByRole("link", { name: /修复/ })).toBeInTheDocument();
    expect(within(topics).getByRole("link", { name: /Claude 连服务器/ })).toBeInTheDocument();
  });
});
