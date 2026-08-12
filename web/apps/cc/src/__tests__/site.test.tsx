import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";

import { App } from "@/App";

function renderApp(path = "/") {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <App />
    </MemoryRouter>,
  );
}

afterEach(() => {
  vi.unstubAllEnvs();
});

describe("首页", () => {
  it("讲清防封与双向同步两个价值点", () => {
    renderApp("/");

    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("安心使用 Claude Code");
    expect(screen.getByRole("heading", { name: "防封方案" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "双向安全同步" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "下载 macOS 版" })).toHaveAttribute(
      "href",
      "/download",
    );
  });
});

describe("定价页", () => {
  it("单一套餐 ¥68/月，订阅按钮直达 Sub2API 收银台", async () => {
    renderApp("/pricing");

    expect(screen.getByText("¥68")).toBeInTheDocument();
    expect(screen.getByText(/首月免费（每个账号限一次）/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "立即订阅" })).toHaveAttribute(
      "href",
      "https://api.lumio.games/purchase",
    );
  });

  it("FAQ 手风琴同时只展开一项", async () => {
    renderApp("/pricing");

    const question = screen.getByRole("button", { name: /有没有免费版/ });
    expect(question).toHaveAttribute("aria-expanded", "false");

    await userEvent.click(question);

    expect(question).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByText(/免费试用一个月/)).toBeInTheDocument();
  });
});

describe("下载页", () => {
  it("配置了下载地址时给出 macOS 安装包链接", () => {
    vi.stubEnv("VITE_CC_DOWNLOAD_ARM_URL", "https://dl.example.com/CCHaven-arm64.dmg");
    vi.stubEnv("VITE_CC_DOWNLOAD_INTEL_URL", "https://dl.example.com/CCHaven-x64.dmg");

    renderApp("/download");

    expect(screen.getByRole("link", { name: /Apple Silicon/ })).toHaveAttribute(
      "href",
      "https://dl.example.com/CCHaven-arm64.dmg",
    );
    expect(screen.getByRole("link", { name: /Intel/ })).toHaveAttribute(
      "href",
      "https://dl.example.com/CCHaven-x64.dmg",
    );
  });

  it("未配置下载地址时给出空态而不是坏链接", () => {
    renderApp("/download");

    expect(screen.getByText(/下载地址尚未配置/)).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: /Apple Silicon/ })).not.toBeInTheDocument();
  });
});

describe("账号入口", () => {
  it("产品站不做登录，账号链接一律回门户并带 next", () => {
    renderApp("/pricing");

    const login = screen.getByRole("link", { name: "登录" });
    expect(login.getAttribute("href")).toMatch(/^https:\/\/lumiogame\.com\/login\?next=/);
    expect(screen.getByRole("link", { name: "注册" }).getAttribute("href")).toMatch(
      /^https:\/\/lumiogame\.com\/signup\?next=/,
    );
  });
});
