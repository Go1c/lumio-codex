import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { describe, expect, it } from "vitest";

import { SiteShell } from "../components/SiteShell";
import { portalAccountLinks, siteUrl } from "../config";

function renderShell(ui: React.ReactElement) {
  return render(<MemoryRouter>{ui}</MemoryRouter>);
}

function hexToRgb(hex: string): { r: number; g: number; b: number } {
  const raw = hex.replace("#", "");
  return {
    r: Number.parseInt(raw.slice(0, 2), 16),
    g: Number.parseInt(raw.slice(2, 4), 16),
    b: Number.parseInt(raw.slice(4, 6), 16),
  };
}

function relativeLuminance(hex: string): number {
  const channel = (value: number) => {
    const srgb = value / 255;
    return srgb <= 0.03928 ? srgb / 12.92 : ((srgb + 0.055) / 1.055) ** 2.4;
  };
  const { r, g, b } = hexToRgb(hex);
  return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
}

function contrastRatio(foreground: string, background: string): number {
  const lighter = Math.max(relativeLuminance(foreground), relativeLuminance(background));
  const darker = Math.min(relativeLuminance(foreground), relativeLuminance(background));
  return (lighter + 0.05) / (darker + 0.05);
}

function cssColorToHex(value: string): string {
  const hex = value.trim().match(/^#([0-9a-f]{6})$/i);
  if (hex) return `#${hex[1].toLowerCase()}`;
  const rgb = value.match(/rgba?\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)/i);
  if (!rgb) {
    throw new Error(`unrecognized color: ${value}`);
  }
  const toHex = (channel: string) => Number(channel).toString(16).padStart(2, "0");
  return `#${toHex(rgb[1])}${toHex(rgb[2])}${toHex(rgb[3])}`;
}

describe("SiteShell · 产品站", () => {
  it("产品站壳品牌是 BestCodex，分段控件换整页", () => {
    renderShell(
      <SiteShell
        brand={{ name: "BestCodex" }}
        site="codex"
        accountLinks={portalAccountLinks("https://codex.bestcodex.app/")}
      >
        <p>内容</p>
      </SiteShell>,
    );

    expect(screen.getByRole("link", { name: /^BestCodex$/ })).toBeInTheDocument();
    expect(screen.getByRole("navigation", { name: "产品" })).toBeInTheDocument();

    const codex = screen.getByRole("link", { name: "Codex" });
    const claude = screen.getByRole("link", { name: "Claude" });
    expect(codex).toHaveAttribute("href", "/codex");
    expect(claude).toHaveAttribute("href", "/claude");
    expect(codex).toHaveAttribute("aria-current", "page");
    expect(claude).not.toHaveAttribute("aria-current");
    expect(claude).not.toHaveAttribute("hidden");
    expect(screen.getByText("内容")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "客服与反馈" })).toBeInTheDocument();
  });

  it("顶栏下载按钮可访问且前景与背景有对比", () => {
    renderShell(
      <SiteShell
        brand={{ name: "BestCodex" }}
        site="cc"
        accountLinks={portalAccountLinks("https://cc.bestcodex.app/")}
      >
        <p>内容</p>
      </SiteShell>,
    );

    const download = screen.getByRole("link", { name: "下载" });
    expect(download).toBeVisible();
    expect(download.textContent?.trim()).toBe("下载");

    const style = window.getComputedStyle(download);
    const foreground = cssColorToHex(style.color);
    const background = cssColorToHex(style.backgroundColor);
    expect(contrastRatio(foreground, background)).toBeGreaterThanOrEqual(4.5);
  });

  it("产品站页脚声明与 OpenAI、Anthropic 无从属", () => {
    renderShell(
      <SiteShell
        brand={{ name: "BestCodex" }}
        site="codex"
        accountLinks={portalAccountLinks("https://codex.bestcodex.app/")}
      >
        <p>内容</p>
      </SiteShell>,
    );

    expect(screen.getByText(/与 OpenAI、Anthropic 无从属/)).toBeInTheDocument();
  });
});

describe("SiteShell · 账号入口", () => {
  it("未登录时账号入口跳门户并带 next 回跳", () => {
    renderShell(
      <SiteShell
        brand={{ name: "BestCodex" }}
        site="cc"
        accountLinks={portalAccountLinks("https://cc.bestcodex.app/pricing")}
        account={{ status: "anonymous" }}
      >
        <p>内容</p>
      </SiteShell>,
    );

    expect(screen.getByRole("link", { name: "登录" })).toHaveAttribute(
      "href",
      "https://bestcodex.app/login?next=https%3A%2F%2Fcc.bestcodex.app%2Fpricing",
    );
    expect(screen.getByRole("link", { name: "注册" })).toHaveAttribute(
      "href",
      "https://bestcodex.app/signup?next=https%3A%2F%2Fcc.bestcodex.app%2Fpricing",
    );
  });

  it("已登录时显示账户入口与邮箱首字母", () => {
    renderShell(
      <SiteShell
        brand={{ name: "BestCodex" }}
        site="cc"
        accountLinks={portalAccountLinks("https://cc.bestcodex.app/")}
        account={{ status: "authenticated", email: "user@example.com" }}
      >
        <p>内容</p>
      </SiteShell>,
    );

    const account = screen.getByRole("link", { name: /账户/ });
    expect(account).toHaveAttribute(
      "href",
      "https://bestcodex.app/account?next=https%3A%2F%2Fcc.bestcodex.app%2F",
    );
    expect(screen.getByText("U")).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "登录" })).not.toBeInTheDocument();
  });

  it("会话尚未确定时不闪现登录入口", () => {
    renderShell(
      <SiteShell
        brand={{ name: "BestCodex" }}
        site="cc"
        accountLinks={portalAccountLinks("https://cc.bestcodex.app/")}
        account={{ status: "loading" }}
      >
        <p>内容</p>
      </SiteShell>,
    );

    expect(screen.queryByRole("link", { name: "登录" })).not.toBeInTheDocument();
    expect(screen.queryByRole("link", { name: /账户/ })).not.toBeInTheDocument();
  });
});

describe("SiteShell · 门户", () => {
  it("传入 BestCodex 时顶栏与页脚都用 BestCodex 标，不用 Lumio 字标", () => {
    renderShell(
      <SiteShell
        brand={{ name: "BestCodex" }}
        site="portal"
        nav={[{ label: "产品", href: siteUrl("codex") }]}
        accountLinks={{ login: "/login", signup: "/signup", account: "/account" }}
      >
        <p>门户内容</p>
      </SiteShell>,
    );

    expect(document.querySelector(".site-header .logo")?.textContent).toMatch(/BestCodex/);
    expect(screen.queryByRole("link", { name: /^Lumio$/ })).not.toBeInTheDocument();
    expect(screen.queryAllByText(/Lumio/)).toHaveLength(0);
    expect(document.querySelector('img[src="/bestcodex-icon.jpg"]')).not.toBeNull();
    expect(screen.getByRole("link", { name: "产品" })).toHaveAttribute(
      "href",
      "https://bestcodex.app/codex",
    );
  });

  it("门户仍用传入的品牌名，不改成 BestCodex", () => {
    renderShell(
      <SiteShell
        brand={{ name: "Lumio" }}
        site="portal"
        nav={[{ label: "BestCodex", href: siteUrl("codex") }]}
        accountLinks={{ login: "/login", signup: "/signup", account: "/account" }}
      >
        <p>门户内容</p>
      </SiteShell>,
    );

    expect(screen.getByRole("link", { name: /^Lumio$/ })).toBeInTheDocument();
    expect(screen.queryByRole("navigation", { name: "产品" })).not.toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Lumio Codex" })).not.toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "CC避风港" })).not.toBeInTheDocument();
    expect(
      screen.getAllByRole("link", { name: "BestCodex" }).some(
        (link) => link.getAttribute("href") === "https://bestcodex.app/codex",
      ),
    ).toBe(true);
  });
});
